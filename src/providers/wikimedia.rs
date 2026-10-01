//! Wikimedia (Wikipedia) search adapter (`P1_VERTICAL`, knowledge profile).
//!
//! Wikipedia is the best disambiguation and definitional source on the web, and
//! its MediaWiki API is public and documented. It does not rank general web
//! results, so it lives in its own `knowledge` profile: useful when the question
//! is "what is X", useless when the question is "who does X now".

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{
    SearchProvider, api_client, build_result, map_api_status, offset_of, retry_after_of,
};

const ENGINE: &str = "wikimedia";
const ENDPOINT: &str = "https://en.wikipedia.org/w/api.php";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    query: RawQuery,
}

#[derive(Deserialize, Debug, Default)]
struct RawQuery {
    #[serde(default)]
    search: Vec<RawHit>,
}

#[derive(Deserialize, Debug, Default)]
struct RawHit {
    #[serde(default)]
    title: String,
    #[serde(default)]
    pageid: Option<u64>,
    #[serde(default)]
    snippet: String,
}

/// Wikipedia returns the snippet with `<span class="searchmatch">` highlight
/// markup, which is display decoration and must not reach the MCP payload.
fn strip_highlights(raw: &str) -> String {
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
    // `&quot;` and friends survive the tag strip.
    out.replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&#39;", "'")
}

/// Canonical article URL from the page id, which survives title changes.
fn canonical_url(hit: &RawHit) -> Option<String> {
    let title = hit.title.trim();
    if title.is_empty() {
        return None;
    }
    Some(match hit.pageid {
        Some(id) => format!("https://en.wikipedia.org/?curid={id}"),
        None => format!("https://en.wikipedia.org/wiki/{}", title.replace(' ', "_")),
    })
}

/// Parse a MediaWiki `list=search` payload into compact results (pure).
pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .query
        .search
        .into_iter()
        .filter_map(|hit| {
            let url = canonical_url(&hit)?;
            let title = hit.title.trim();
            Some(build_result(
                ENGINE,
                &url,
                title,
                &strip_highlights(&hit.snippet),
            ))
        })
        .collect())
}

/// Wikipedia provider over the shared public-API HTTP client.
pub struct WikimediaProvider {
    config: Config,
}

impl WikimediaProvider {
    /// Create the provider for the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for WikimediaProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let limit = limit.clamp(1, 50);
        let offset = offset_of(page, limit);
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("action", "query".to_string()),
                ("list", "search".to_string()),
                ("srsearch", query.to_string()),
                ("srlimit", limit.to_string()),
                ("sroffset", offset.to_string()),
                ("format", "json".to_string()),
                ("formatversion", "2".to_string()),
            ])
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
                ("action", "query".to_string()),
                ("list", "search".to_string()),
                ("srsearch", "rust".to_string()),
                ("srlimit", "1".to_string()),
                ("format", "json".to_string()),
            ])
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
      "batchcomplete": "",
      "query": {
        "searchinfo": {"totalhits": 4735},
        "search": [
          {
            "ns": 0,
            "title": "Rust (programming language)",
            "pageid": 29414838,
            "snippet": "<span class=\"searchmatch\">Rust</span> is a general-purpose <span class=\"searchmatch\">programming</span> language &quot;focused&quot; on safety."
          },
          { "ns": 0, "title": "Rust (band)", "pageid": 999 }
        ]
      }
    }"#;

    #[test]
    fn strips_highlight_markup_and_entities() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 2);
        let snippet = &results[0].snippet;
        assert!(!snippet.contains('<'), "markup must be removed: {snippet}");
        assert!(!snippet.contains("&quot;"));
        assert!(snippet.starts_with("Rust is a general-purpose programming language"));
        assert!(snippet.contains("\"focused\""));
    }

    #[test]
    fn uses_pageid_urls() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results[0].url, "https://en.wikipedia.org/?curid=29414838");
        assert_eq!(results[0].title, "Rust (programming language)");
    }

    #[test]
    fn falls_back_to_a_title_url_without_pageid() {
        let parsed: RawResponse =
            flojo_mcp::serde_json::from_str(r#"{"query":{"search":[{"title":"Ada Lovelace"}]}}"#)
                .expect("fixture");
        let url = canonical_url(&parsed.query.search[0]).expect("url");
        assert_eq!(url, "https://en.wikipedia.org/wiki/Ada_Lovelace");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("<html/>").is_err());
        assert!(parse_results("{\"query\": {}}").is_ok_and(|r| r.is_empty()));
    }
}
