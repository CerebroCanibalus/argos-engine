//! OpenAlex public Works API provider (keyless, `P1_PUBLIC_API`).
//!
//! OpenAlex exposes an open bibliographic index (papers, DOIs, authors,
//! citation counts and abstracts) over a stable, documented JSON API. It needs
//! no API key, which makes it a first-class academic provider instead of a
//! user-key expense.
//!
//! Measured 2026-09-25: anonymous traffic works but OpenAlex applies a load
//! dependent rate limit. A 429 may carry the requested delay in a JSON
//! `retryAfter` field instead of the HTTP header, so both are read.

use std::collections::BTreeMap;

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;
use flojo_mcp::truncate_string;

use crate::config::Config;
use crate::error::ArgosError;
use crate::limits::{SNIPPET_MAX_CHARS, TITLE_MAX_CHARS};
use crate::providers::{SearchProvider, api_client, compact_source, parse_retry_after};
use crate::types::SearchResult;

const ENGINE: &str = "openalex";
const ENDPOINT: &str = "https://api.openalex.org/works";

/// `works` response envelope (unknown fields are ignored).
#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    results: Vec<RawWork>,
}

/// Error payload used by OpenAlex to report the delay it wants us to wait.
#[derive(Deserialize, Debug, Default)]
struct RawRateLimit {
    #[serde(rename = "retryAfter", default)]
    retry_after_secs: Option<u64>,
}

#[derive(Deserialize, Debug, Default)]
struct RawWork {
    #[serde(default)]
    id: String,
    #[serde(default)]
    doi: Option<String>,
    // OpenAlex sends explicit JSON `null` for several of these on records with
    // sparse metadata, so every optional field is modelled as `Option`.
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    publication_year: Option<i32>,
    #[serde(default)]
    cited_by_count: Option<u32>,
    #[serde(default)]
    authorships: Option<Vec<RawAuthorship>>,
    #[serde(default)]
    abstract_inverted_index: Option<BTreeMap<String, Vec<u32>>>,
}

#[derive(Deserialize, Debug)]
struct RawAuthorship {
    #[serde(default)]
    author: Option<RawAuthor>,
}

#[derive(Deserialize, Debug)]
struct RawAuthor {
    #[serde(default)]
    display_name: Option<String>,
}

/// Rebuild the plain-text abstract from OpenAlex's inverted index.
///
/// The API ships abstracts as `{word: [positions]}` to save space; reconstructing
/// them by sorted position is lossless and far cheaper for the agent than
/// shipping the inverted structure through the MCP payload.
fn flatten_abstract(index: Option<&BTreeMap<String, Vec<u32>>>) -> String {
    let Some(index) = index else {
        return String::new();
    };
    let mut slots: Vec<(u32, &str)> = index
        .iter()
        .flat_map(|(word, positions)| {
            positions
                .iter()
                .map(move |position| (*position, word.as_str()))
        })
        .collect();
    slots.sort_unstable();
    slots
        .into_iter()
        .map(|(_, word)| word)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Author surnames, capped: a full author list would burn the snippet budget.
fn author_list(authorships: Option<&Vec<RawAuthorship>>) -> String {
    let Some(authorships) = authorships else {
        return String::new();
    };
    authorships
        .iter()
        .filter_map(|authorship| {
            let name = authorship.author.as_ref()?.display_name.as_deref()?;
            let trimmed = name.trim();
            (!trimmed.is_empty()).then_some(trimmed)
        })
        .take(3)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parse an OpenAlex `works` payload into compact results (pure, fixture-testable).
pub fn parse_results(payload: &str) -> Result<Vec<SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .results
        .into_iter()
        .filter_map(|work| {
            let title = work
                .display_name
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_string();
            // Prefer the DOI resolver: it is the canonical, stable landing page.
            let url = work
                .doi
                .filter(|doi| !doi.trim().is_empty())
                .unwrap_or(work.id);
            if url.trim().is_empty() || title.is_empty() {
                return None;
            }
            let mut snippet = String::new();
            let authors = author_list(work.authorships.as_ref());
            if !authors.is_empty() {
                snippet.push_str(&authors);
                snippet.push_str(". ");
            }
            if let Some(year) = work.publication_year {
                snippet.push_str(&format!("{year}. "));
            }
            if let Some(citations) = work.cited_by_count {
                snippet.push_str(&format!("Cited {citations} times. "));
            }
            let abstract_text = flatten_abstract(work.abstract_inverted_index.as_ref());
            if !abstract_text.is_empty() {
                snippet.push_str(&abstract_text);
            }
            // Sparse records can carry no authors, year, citations or abstract.
            // An empty snippet would waste a result slot, so fall back to the
            // title rather than returning a blank payload.
            if snippet.trim().is_empty() {
                snippet = title.clone();
            }
            Some(SearchResult {
                source: compact_source(&url),
                url,
                title: truncate_string(&title, TITLE_MAX_CHARS).0,
                snippet: truncate_string(snippet.trim(), SNIPPET_MAX_CHARS).0,
                engine: Some(ENGINE.into()),
            })
        })
        .collect())
}

/// OpenAlex provider over the shared public-API HTTP client.
pub struct OpenAlexProvider {
    config: Config,
}

impl OpenAlexProvider {
    /// Create the provider for the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for OpenAlexProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 50);
        let mut query = vec![
            ("search", query.to_string()),
            ("per-page", per_page.to_string()),
            ("page", page.max(1).to_string()),
        ];
        // OpenAlex asks API clients for a contact address to enter its polite
        // pool; this is not a key and creates no account.
        if let Some(email) = &self.config.contact_email {
            query.push(("mailto", email.clone()));
        }
        let response = api_client()
            .get(ENDPOINT)
            .query(&query)
            .timeout(self.config.request_timeout)
            .send()
            .await
            .map_err(|ex| ArgosError::Unreachable {
                origin: ENGINE.into(),
                cause: ex.to_string(),
            })?;

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            if code == 429 || code == 403 {
                let header_retry = response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok())
                    .and_then(parse_retry_after);
                // OpenAlex documents a JSON `retryAfter` on 429 bodies; read it
                // so the shared cooldown matches what the server asked for.
                let retry_after_secs = if header_retry.is_some() || code != 429 {
                    header_retry
                } else {
                    response.text().await.ok().and_then(|body| {
                        flojo_mcp::serde_json::from_str::<RawRateLimit>(&body)
                            .ok()
                            .and_then(|payload| payload.retry_after_secs)
                    })
                };
                return Err(ArgosError::RateLimited {
                    engine: ENGINE.into(),
                    status: code,
                    retry_after_secs,
                });
            }
            return Err(ArgosError::Unreachable {
                origin: ENGINE.into(),
                cause: format!("HTTP {code}"),
            });
        }

        let payload = response
            .text()
            .await
            .map_err(|ex| ArgosError::Unreachable {
                origin: ENGINE.into(),
                cause: ex.to_string(),
            })?;
        let mut results = parse_results(&payload)?;
        results.truncate(limit);
        Ok(results)
    }

    async fn health(&self) -> bool {
        match api_client()
            .get(ENDPOINT)
            .query(&[("per-page", "1"), ("page", "1")])
            .timeout(self.config.ping_timeout)
            .send()
            .await
        {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "results": [
        {
          "id": "https://openalex.org/W1",
          "doi": "https://doi.org/10.1000/retrieval",
          "display_name": "Retrieval Augmented Generation for Knowledge-Intensive NLP",
          "publication_year": 2020,
          "cited_by_count": 12000,
          "authorships": [
            {"author": {"display_name": "Patrick Lewis"}},
            {"author": {"display_name": "Ethan Perez"}},
            {"author": {"display_name": "Alec Radford"}},
            {"author": {"display_name": "Too Many"}}
          ],
          "abstract_inverted_index": {
            "augmented": [1],
            "generation": [2],
            "retrieval": [0]
          }
        },
        {
          "id": "https://openalex.org/W2",
          "display_name": "Unrelated work without DOI",
          "publication_year": 2019
        },
        {
          "id": "https://openalex.org/W3",
          "doi": "",
          "display_name": ""
        }
      ]
    }"#;

    #[test]
    fn parses_openalex_contract() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        // The third record has no usable title and is dropped.
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://doi.org/10.1000/retrieval");
        assert_eq!(results[0].engine.as_deref(), Some(ENGINE));
        assert_eq!(results[0].source, "doi.org");
        // Entry 2 has no DOI, so the OpenAlex id is the landing page.
        assert_eq!(results[1].url, "https://openalex.org/W2");
    }

    #[test]
    fn rebuilds_abstract_in_position_order() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert!(
            results[0]
                .snippet
                .starts_with("Patrick Lewis, Ethan Perez, Alec Radford."),
            "snippet must lead with capped authors: {}",
            results[0].snippet
        );
        assert!(results[0].snippet.contains("2020"));
        assert!(results[0].snippet.contains("Cited 12000 times."));
        assert!(
            results[0]
                .snippet
                .ends_with("retrieval augmented generation"),
            "abstract must be rebuilt by position: {}",
            results[0].snippet
        );
    }

    #[test]
    fn tolerates_sparse_records_with_explicit_nulls() {
        // Live records send `null` for absent abstract/authorship/name fields;
        // a strict field type aborts the whole response instead of skipping
        // one sparse work, which silently emptied whole searches.
        const SPARSE: &str = r#"{
          "results": [
            {
              "id": "https://openalex.org/W9",
              "doi": null,
              "display_name": "Sparse but usable",
              "publication_year": null,
              "cited_by_count": null,
              "authorships": null,
              "abstract_inverted_index": null
            },
            {
              "id": "https://openalex.org/W8",
              "doi": null,
              "display_name": null,
              "authorships": null
            }
          ]
        }"#;
        let results = parse_results(SPARSE).expect("sparse records must not abort parsing");
        assert_eq!(results.len(), 1, "only the titled record is usable");
        assert_eq!(results[0].url, "https://openalex.org/W9");
        assert_eq!(results[0].snippet, "Sparse but usable");
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(parse_results("not json at all").is_err());
        assert!(parse_results("{\"unexpected\": true}").is_ok_and(|r| r.is_empty()));
    }
}
