//! Semantic Scholar Graph API adapter (keyless by default, `P1_PUBLIC_API`).
//!
//! Semantic Scholar indexes a very large citation graph and offers a documented
//! public API. Anonymous traffic works but is aggressively throttled, especially
//! from shared addresses, so Argos treats 429 as a normal cooldown rather than
//! an error. Setting `ARGOS_SEMANTIC_SCHOLAR_KEY` raises the ceiling; no key is
//! required for it to be useful.
//!
//! Measured 2026-10-01: this host's IP already receives 429 on anonymous calls,
//! so the adapter is registered at lower priority than OpenAlex and Crossref.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{
    SearchProvider, api_client, build_result, map_api_status, offset_of, retry_after_of,
};

const ENGINE: &str = "semantic_scholar";
const ENDPOINT: &str = "https://api.semanticscholar.org/graph/v1/paper/search";

/// Fields requested explicitly: the API returns nothing else.
const FIELDS: &str = "title,abstract,authors,year,citationCount,venue,externalIds,url";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    data: Vec<RawPaper>,
}

#[derive(Deserialize, Debug, Default)]
struct RawPaper {
    #[serde(default)]
    title: Option<String>,
    // `abstract` is a Rust keyword; the wire name is kept via rename.
    #[serde(default, rename = "abstract")]
    raw_abstract: Option<String>,
    #[serde(default)]
    year: Option<i32>,
    #[serde(rename = "citationCount", default)]
    citation_count: Option<u32>,
    #[serde(default)]
    venue: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(rename = "externalIds", default)]
    external_ids: ExternalIds,
    #[serde(default)]
    authors: Vec<RawAuthor>,
}

#[derive(Deserialize, Debug, Default)]
struct ExternalIds {
    #[serde(rename = "DOI", default)]
    doi: Option<String>,
    #[serde(rename = "ArXiv", default)]
    arxiv: Option<String>,
}

#[derive(Deserialize, Debug)]
struct RawAuthor {
    #[serde(default)]
    name: String,
}

/// Canonical landing page, preferring the DOI and falling back to the arXiv id.
fn canonical_url(paper: &RawPaper) -> Option<String> {
    if let Some(doi) = paper
        .external_ids
        .doi
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        return Some(format!("https://doi.org/{doi}"));
    }
    if let Some(arxiv) = paper
        .external_ids
        .arxiv
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty())
    {
        return Some(format!("https://arxiv.org/abs/{arxiv}"));
    }
    paper
        .url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_string)
}

/// Parse a Semantic Scholar `paper/search` payload into compact results (pure).
pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .data
        .into_iter()
        .filter_map(|paper| {
            let url = canonical_url(&paper)?;
            let title = paper.title.as_deref().unwrap_or_default().trim();
            if title.is_empty() {
                return None;
            }
            let mut snippet = String::new();
            let authors = paper
                .authors
                .iter()
                .map(|a| a.name.trim())
                .filter(|name| !name.is_empty())
                .take(3)
                .collect::<Vec<_>>()
                .join(", ");
            if !authors.is_empty() {
                snippet.push_str(&authors);
                snippet.push_str(". ");
            }
            if let Some(venue) = paper
                .venue
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                snippet.push_str(&format!("{venue}. "));
            }
            if let Some(year) = paper.year {
                snippet.push_str(&format!("{year}. "));
            }
            if let Some(citations) = paper.citation_count {
                snippet.push_str(&format!("Cited {citations} times. "));
            }
            if let Some(abstract_text) = paper
                .raw_abstract
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())
            {
                snippet.push_str(abstract_text);
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

/// Semantic Scholar provider over the shared public-API HTTP client.
pub struct SemanticScholarProvider {
    config: Config,
}

impl SemanticScholarProvider {
    /// Create the provider for the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for SemanticScholarProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 100);
        let offset = offset_of(page, per_page);
        let mut params = vec![
            ("query", query.to_string()),
            ("limit", per_page.to_string()),
            ("offset", offset.to_string()),
            ("fields", FIELDS.to_string()),
        ];
        if let Some(key) = &self.config.semantic_scholar_key {
            params.push(("apiKey", key.clone()));
        }
        let response = api_client()
            .get(ENDPOINT)
            .query(&params)
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
            return Err(map_api_status(ENGINE, code, retry_after_of(&response)));
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
            .query(&[
                ("query", "electron".to_string()),
                ("limit", "1".to_string()),
                ("fields", "title".to_string()),
            ])
            .timeout(self.config.ping_timeout)
            .send()
            .await
        {
            // A 429 still proves the endpoint is reachable and answering; the
            // throttle is reported per search, not as an outage.
            Ok(response) => {
                let code = response.status().as_u16();
                code != 404 && code != 503
            }
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "total": 100,
      "data": [
        {
          "title": "Gated recurrent units revisited",
          "year": 2026,
          "venue": "Neural Computation",
          "citationCount": 42,
          "abstract": "We revisit gated recurrent units.",
          "externalIds": {"DOI": "10.1000/gru", "ArXiv": "2301.00001"},
          "url": "https://www.semanticscholar.org/paper/abc",
          "authors": [{"name": "Ada Lovelace"}, {"name": "Alan Turing"}, {"name": "Three"}, {"name": "Four"}]
        },
        {
          "title": "Only an arXiv identifier",
          "year": 2025,
          "externalIds": {"ArXiv": "2302.00002"}
        },
        { "title": "No identifier at all" }
      ]
    }"#;

    #[test]
    fn prefers_doi_then_arxiv() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 2, "a paper with no identifier is unusable");
        assert_eq!(results[0].url, "https://doi.org/10.1000/gru");
        assert_eq!(results[1].url, "https://arxiv.org/abs/2302.00002");
        assert_eq!(results[0].source, "doi.org");
    }

    #[test]
    fn snippet_is_capped_and_ordered() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        let snippet = &results[0].snippet;
        assert!(snippet.starts_with("Ada Lovelace, Alan Turing, Three."));
        assert!(!snippet.contains("Four"));
        assert!(snippet.contains("Neural Computation. 2026. Cited 42 times."));
        assert!(snippet.ends_with("We revisit gated recurrent units."));
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("rate limited").is_err());
        assert!(parse_results("{\"data\": []}").is_ok_and(|r| r.is_empty()));
        assert_eq!(offset_of(2, 5), 5);
    }
}
