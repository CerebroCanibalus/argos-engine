//! Crossref REST API provider (keyless, `P1_PUBLIC_API`).
//!
//! Crossref is the DOI registration agency: it owns the canonical metadata for
//! the overwhelming majority of the scholarly literature. The REST API needs no
//! key and is explicitly documented as the metadata source of record, which
//! makes it the best second opinion when OpenAlex misses a work.
//!
//! Abstracts arrive as JATS XML fragments inside JSON, so tags are stripped
//! before the text reaches the token budget.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;
use flojo_mcp::truncate_string;

use crate::config::Config;
use crate::error::ArgosError;
use crate::limits::{SNIPPET_MAX_CHARS, TITLE_MAX_CHARS};
use crate::providers::{SearchProvider, api_client, compact_source, parse_retry_after};
use crate::types::SearchResult;

const ENGINE: &str = "crossref";
const ENDPOINT: &str = "https://api.crossref.org/works";

/// Crossref `message` envelope (unknown fields are ignored).
#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    message: RawMessage,
}

#[derive(Deserialize, Debug, Default)]
struct RawMessage {
    #[serde(default)]
    items: Vec<RawItem>,
}

#[derive(Deserialize, Debug, Default)]
struct RawItem {
    // Crossref's wire format uses upper-case `DOI`/`URL`; serde is
    // case-sensitive, so the rename is mandatory, not cosmetic.
    #[serde(default, rename = "DOI")]
    doi: String,
    #[serde(default, rename = "URL")]
    url: String,
    #[serde(default)]
    title: Vec<String>,
    #[serde(default, rename = "container-title")]
    container_title: Vec<String>,
    #[serde(default, rename = "abstract")]
    raw_abstract: String,
    #[serde(default)]
    author: Vec<RawAuthor>,
    #[serde(default, rename = "is-referenced-by-count")]
    is_referenced_by_count: Option<u32>,
    #[serde(default)]
    publisher: String,
}

#[derive(Deserialize, Debug)]
struct RawAuthor {
    #[serde(default)]
    given: String,
    #[serde(default)]
    family: String,
}

/// Strip JATS/XML tags and collapse whitespace from a Crossref abstract.
fn clean_abstract(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut inside_tag = false;
    for character in raw.chars() {
        match character {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => out.push(character),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "Family, Given" display, falling back to the family name alone.
fn display_name(author: &RawAuthor) -> String {
    let given = author.given.trim();
    let family = author.family.trim();
    match (given.is_empty(), family.is_empty()) {
        (true, true) => String::new(),
        (true, false) => family.to_string(),
        (false, true) => given.to_string(),
        (false, false) => format!("{family}, {given}"),
    }
}

fn author_line(authors: &[RawAuthor]) -> String {
    authors
        .iter()
        .map(display_name)
        .filter(|name| !name.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join("; ")
}

/// Parse a Crossref `works` payload into compact results (pure, fixture-testable).
pub fn parse_results(payload: &str) -> Result<Vec<SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .message
        .items
        .into_iter()
        .filter_map(|item| {
            let title = item.title.first()?.trim().to_string();
            // Prefer the DOI resolver, then whatever Crossref recorded.
            let url = if !item.doi.trim().is_empty() {
                format!("https://doi.org/{}", item.doi.trim())
            } else if !item.url.trim().is_empty() {
                item.url.trim().to_string()
            } else {
                return None;
            };
            if title.is_empty() {
                return None;
            }

            let mut snippet = String::new();
            let authors = author_line(&item.author);
            if !authors.is_empty() {
                snippet.push_str(&authors);
                snippet.push_str(". ");
            }
            if let Some(venue) = item.container_title.first().map(|value| value.trim())
                && !venue.is_empty()
            {
                snippet.push_str(&format!("{venue}. "));
            }
            if let Some(citations) = item.is_referenced_by_count {
                snippet.push_str(&format!("Cited {citations} times. "));
            }
            if !item.publisher.trim().is_empty() {
                snippet.push_str(&format!("{}. ", item.publisher.trim()));
            }
            let abstract_text = clean_abstract(&item.raw_abstract);
            if !abstract_text.is_empty() {
                snippet.push_str(&abstract_text);
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

/// Crossref provider over the shared public-API HTTP client.
pub struct CrossrefProvider {
    config: Config,
}

impl CrossrefProvider {
    /// Create the provider for the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for CrossrefProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<SearchResult>, ArgosError> {
        let rows = limit.clamp(1, 50);
        let offset = page.saturating_sub(1).saturating_mul(rows);
        // Crossref's `/works` rejects the `page` parameter with HTTP 400
        // (verified live 2026-10-01); pagination here is `offset`-based.
        let mut query = vec![
            ("query", query.to_string()),
            ("rows", rows.to_string()),
            ("offset", offset.to_string()),
        ];
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
                let retry_after_secs = response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok())
                    .and_then(parse_retry_after);
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
            .query(&[("rows", "1".to_string()), ("offset", "0".to_string())])
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
      "status": "ok",
      "message": {
        "total-results": 2,
        "items": [
          {
            "DOI": "10.1000/xyz",
            "URL": "http://dx.doi.org/10.1000/xyz",
            "title": ["Retrieval Augmented Generation"],
            "container-title": ["Journal of Testing"],
            "publisher": "Test Press",
            "is-referenced-by-count": 42,
            "author": [{"given": "Ada", "family": "Lovelace"}, {"given": "Alan", "family": "Turing"}],
            "abstract": "<jats:p>We study <jats:italic>generation</jats:italic> with retrieval.</jats:p>"
          },
          {
            "DOI": "10.1000/second",
            "title": ["Second work without URL field"]
          },
          {
            "title": ["No DOI and no URL"],
            "URL": ""
          }
        ]
      }
    }"#;

    #[test]
    fn parses_crossref_contract() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        // The third record has neither DOI nor URL and is dropped.
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://doi.org/10.1000/xyz");
        assert_eq!(results[0].source, "doi.org");
        assert_eq!(results[0].engine.as_deref(), Some(ENGINE));
        assert_eq!(results[1].url, "https://doi.org/10.1000/second");
    }

    #[test]
    fn strips_jats_from_abstract_and_caps_authors() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        let snippet = &results[0].snippet;
        assert!(snippet.starts_with("Lovelace, Ada; Turing, Alan."));
        assert!(snippet.contains("Journal of Testing"));
        assert!(snippet.contains("Cited 42 times."));
        assert!(snippet.ends_with("We study generation with retrieval."));
        assert!(
            !snippet.contains('<'),
            "JATS tags must never leak: {snippet}"
        );
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(parse_results("<html>nope</html>").is_err());
        assert!(parse_results("{\"message\": {}}").is_ok_and(|r| r.is_empty()));
    }
}
