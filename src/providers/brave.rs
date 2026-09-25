//! Brave Search html provider (keyless, independent index, primp-verified).

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::truncate_string;
use primp::StatusCode;

use crate::config::Config;
use crate::error::ArgosError;
use crate::limits::{SNIPPET_MAX_CHARS, TITLE_MAX_CHARS};
use crate::providers::{
    SearchProvider, compact_source, impersonated_client, impersonated_health_client,
};
use crate::types::SearchResult;

const ENGINE: &str = "brave";
const ENDPOINT: &str = "https://search.brave.com/search";
const HOST: &str = "search.brave.com";

/// Brave html provider: independent crawler index (unlike DDG/Bing), which
/// gives the fanout true index diversity on top of separate rate budgets.
pub struct BraveProvider {
    config: Config,
}

impl BraveProvider {
    /// Create the provider for the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

/// Parse a Brave results page (pure - fixture-testable, no I/O).
pub fn parse_results(html: &str) -> Result<Vec<SearchResult>, ArgosError> {
    use scraper::{Html, Selector};

    let document = Html::parse_document(html);
    // Only organic web blocks: ads carry other data-type values.
    let block = Selector::parse("div[data-type=\"web\"]").expect("literal selector");
    let link = Selector::parse("a[href]").expect("literal selector");
    let title_sel = Selector::parse("div.title").expect("literal selector");
    let snippet_sel = Selector::parse(".generic-snippet .content").expect("literal selector");

    let mut results = Vec::new();
    for item in document.select(&block) {
        let Some(anchor) = item.select(&link).next() else {
            continue;
        };
        let Some(href) = anchor.value().attr("href") else {
            continue;
        };
        if !href.starts_with("http") {
            continue;
        }
        let title_node = item.select(&title_sel).next();
        let title = match &title_node {
            Some(node) => node.text().collect::<String>(),
            None => anchor.text().collect::<String>(),
        };
        let title = title.trim().to_string();
        if title.is_empty() {
            continue;
        }
        let snippet = item
            .select(&snippet_sel)
            .next()
            .map(|node| node.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        results.push(SearchResult {
            source: compact_source(href),
            url: href.to_string(),
            title: truncate_string(&title, TITLE_MAX_CHARS).0,
            snippet: truncate_string(&snippet, SNIPPET_MAX_CHARS).0,
            engine: Some(ENGINE.into()),
        });
    }
    Ok(results)
}

#[async_trait]
impl SearchProvider for BraveProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<SearchResult>, ArgosError> {
        let mut params: Vec<(&str, String)> =
            vec![("q", query.to_string()), ("source", "web".to_string())];
        if page > 1 {
            params.push(("offset", (page - 1).to_string()));
        }

        let response = impersonated_client(&self.config)
            .get(ENDPOINT)
            .query(&params)
            .send()
            .await
            .map_err(|ex| ArgosError::Unreachable {
                origin: HOST.into(),
                cause: ex.to_string(),
            })?;

        match response.status() {
            StatusCode::OK => {
                let body = response
                    .text()
                    .await
                    .map_err(|ex| ArgosError::Unreachable {
                        origin: HOST.into(),
                        cause: ex.to_string(),
                    })?;
                let mut results = parse_results(&body)?;
                results.truncate(limit);
                Ok(results)
            }
            status @ (StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS) => {
                Err(ArgosError::RateLimited {
                    engine: ENGINE.into(),
                    status: status.as_u16(),
                })
            }
            status => Err(ArgosError::Unreachable {
                origin: HOST.into(),
                cause: format!("HTTP {}", status.as_u16()),
            }),
        }
    }

    async fn health(&self) -> bool {
        impersonated_health_client()
            .get(ENDPOINT)
            .send()
            .await
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/brave.html");

    #[test]
    fn parses_real_fixture() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert!(
            results.len() >= 8,
            "expected the real page's organic results, got {}",
            results.len()
        );
        for result in &results {
            assert!(result.url.starts_with("http"));
            assert!(!result.title.is_empty());
            assert_eq!(result.engine.as_deref(), Some("brave"));
        }
        // The fixture's first hit is the Rust homepage (captured live).
        assert_eq!(results[0].url, "https://rust-lang.org/");
    }

    #[test]
    fn truncates_to_token_budget() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        for result in &results {
            assert!(result.snippet.chars().count() <= SNIPPET_MAX_CHARS + 3);
            assert!(result.title.chars().count() <= TITLE_MAX_CHARS + 3);
        }
    }

    #[test]
    fn rejects_garbage_html_gracefully() {
        let results =
            parse_results("<html><body>429 too many requests</body></html>").expect("parses");
        assert!(results.is_empty());
    }
}
