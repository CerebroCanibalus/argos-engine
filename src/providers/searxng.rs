//! Local SearXNG JSON API provider (optional adapter - requires the WSL2 stack).

use std::sync::OnceLock;

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;
use flojo_mcp::serde_json;
use flojo_mcp::truncate_string;

use crate::config::Config;
use crate::error::ArgosError;
use crate::limits::{SNIPPET_MAX_CHARS, TITLE_MAX_CHARS};
use crate::providers::SearchProvider;
use crate::stack;
use crate::types::SearchResult;

/// One result as returned by the SearXNG JSON API.
#[derive(Deserialize, Debug)]
struct RawResult {
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    engine: Option<String>,
}

/// SearXNG `/search` JSON payload (unknown fields are ignored).
#[derive(Deserialize, Debug)]
struct RawResponse {
    #[serde(default)]
    results: Vec<RawResult>,
}

/// Parse a SearXNG JSON payload into compact, truncated results (no I/O).
pub fn parse_results(payload: &str) -> Result<Vec<SearchResult>, ArgosError> {
    let parsed: RawResponse =
        serde_json::from_str(payload).map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .results
        .into_iter()
        .map(|raw| SearchResult {
            url: raw.url,
            title: truncate_string(&raw.title, TITLE_MAX_CHARS).0,
            snippet: truncate_string(&raw.content, SNIPPET_MAX_CHARS).0,
            engine: raw.engine,
        })
        .collect())
}

/// SearXNG-backed provider with a shared HTTP client (connection reuse).
pub struct SearxNgProvider {
    config: Config,
}

impl SearxNgProvider {
    /// Create a provider for the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Shared client: built once, reused across calls (no cold start per tool call).
    fn client() -> reqwest::Client {
        static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
        CLIENT
            .get_or_init(|| {
                reqwest::Client::builder()
                    .build()
                    .expect("reqwest default client builder is infallible")
            })
            .clone()
    }

    fn send_search(&self, query: &str, page: usize) -> reqwest::RequestBuilder {
        Self::client()
            .get(format!("{}/search", self.config.searxng_url))
            .query(&[
                ("q", query.to_string()),
                ("format", "json".to_string()),
                ("page", page.to_string()),
                ("safesearch", "0".to_string()),
            ])
            .timeout(self.config.request_timeout)
    }

    fn classify(ex: reqwest::Error, base: &str) -> ArgosError {
        if ex.is_connect() {
            ArgosError::Down {
                base: base.to_string(),
            }
        } else {
            ArgosError::Unreachable {
                origin: format!("SearXNG at {base}"),
                cause: ex.to_string(),
            }
        }
    }
}

#[async_trait]
impl SearchProvider for SearxNgProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<SearchResult>, ArgosError> {
        let base = self.config.searxng_url.clone();

        let response = match self.send_search(query, page).send().await {
            Ok(response) => response,
            Err(ex) if ex.is_connect() && self.config.auto_start => {
                // Connection refused: boot the stack once, then retry.
                stack::ensure_up(&self.config).await?;
                self.send_search(query, page)
                    .send()
                    .await
                    .map_err(|ex| Self::classify(ex, &base))?
            }
            Err(ex) => return Err(Self::classify(ex, &base)),
        };

        if !response.status().is_success() {
            let status = response.status().as_u16();
            return Err(ArgosError::Http {
                status,
                hint: "JSON API disabled? Ensure searxng/settings.yml sets search.formats: [html, json] and restart the container.",
            });
        }

        let payload = response
            .text()
            .await
            .map_err(|ex| ArgosError::Unreachable {
                origin: format!("SearXNG at {base}"),
                cause: ex.to_string(),
            })?;
        let mut results = parse_results(&payload)?;
        results.truncate(limit);

        // The stack served us: refresh the idle watchdog.
        stack::note_usage(&self.config).await;
        Ok(results)
    }

    async fn health(&self) -> bool {
        match Self::client()
            .get(&self.config.searxng_url)
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
    use crate::limits::SNIPPET_MAX_CHARS;

    const FIXTURE: &str = include_str!("../../tests/fixtures/searxng_search.json");

    #[test]
    fn parses_fixture_contract() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert_eq!(results[0].engine.as_deref(), Some("google"));
        assert_eq!(results[1].engine.as_deref(), Some("duckduckgo"));
    }

    #[test]
    fn truncates_long_snippets_to_budget() {
        const ELLIPSIS_CHARS: usize = 3; // truncate_string appends "..." on cut
        let results = parse_results(FIXTURE).expect("fixture must parse");
        let len = results[1].snippet.chars().count();
        assert!(
            len <= SNIPPET_MAX_CHARS + ELLIPSIS_CHARS,
            "snippet must respect the token budget, got {len}"
        );
        assert_eq!(len, SNIPPET_MAX_CHARS + ELLIPSIS_CHARS);
        assert!(results[1].snippet.ends_with("..."));
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("not json at all").is_err());
        assert!(parse_results("{\"unexpected\": true}").is_ok_and(|r| r.is_empty()));
    }
}
