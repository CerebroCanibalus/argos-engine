//! DuckDuckGo html endpoint provider (keyless, no VM).

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::truncate_string;
use percent_encoding::percent_decode_str;
use primp::StatusCode;

use crate::config::Config;
use crate::error::ArgosError;
use crate::limits::{SNIPPET_MAX_CHARS, TITLE_MAX_CHARS};
use crate::providers::{SearchProvider, impersonated_client, impersonated_health_client};
use crate::types::SearchResult;

const ENGINE: &str = "duckduckgo";
const ENDPOINT: &str = "https://html.duckduckgo.com/html/";
const HOST: &str = "html.duckduckgo.com";

/// DuckDuckGo html provider: POST form endpoint, `div.result` blocks.
pub struct DuckDuckGoProvider {
    config: Config,
}

impl DuckDuckGoProvider {
    /// Create the provider for the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

/// Unwrap `//duckduckgo.com/l/?uddg=<percent-encoded>` redirects to the target URL.
fn unwrap_href(href: &str) -> String {
    if let Some(pos) = href.find("uddg=") {
        let value = &href[pos + "uddg=".len()..];
        let end = value.find('&').unwrap_or(value.len());
        if let Ok(decoded) = percent_decode_str(&value[..end]).decode_utf8()
            && decoded.starts_with("http")
        {
            return decoded.to_string();
        }
    }
    if let Some(rest) = href.strip_prefix("//") {
        return format!("https://{rest}");
    }
    href.to_string()
}

/// Parse a DDG html results page (pure - fixture-testable, no I/O).
pub fn parse_results(html: &str) -> Result<Vec<SearchResult>, ArgosError> {
    use scraper::{Html, Selector};

    let document = Html::parse_document(html);
    let block = Selector::parse("div.result").expect("literal selector");
    let link = Selector::parse("a.result__a").expect("literal selector");
    let snippet_sel = Selector::parse("a.result__snippet").expect("literal selector");

    let mut results = Vec::new();
    for item in document.select(&block) {
        let Some(anchor) = item.select(&link).next() else {
            continue;
        };
        let Some(href) = anchor.value().attr("href") else {
            continue;
        };
        // Advertisements and internal links are not organic results.
        if href.contains("/y.js") || href.contains("duckduckgo.com/?q=") {
            continue;
        }
        let title = anchor.text().collect::<String>();
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
            url: unwrap_href(href),
            title: truncate_string(&title, TITLE_MAX_CHARS).0,
            snippet: truncate_string(&snippet, SNIPPET_MAX_CHARS).0,
            engine: Some(ENGINE.into()),
        });
    }
    Ok(results)
}

#[async_trait]
impl SearchProvider for DuckDuckGoProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<SearchResult>, ArgosError> {
        // Same payload ddgs ships: q/b/l, plus the `s` offset for deeper pages.
        let mut params: Vec<(&str, String)> = vec![
            ("q", query.to_string()),
            ("b", String::new()),
            ("l", "us-en".to_string()),
        ];
        if page > 1 {
            let offset = 10 + (page - 2) * 15;
            params.push(("s", offset.to_string()));
        }

        let response = impersonated_client(&self.config)
            .post(ENDPOINT)
            .form(&params)
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
            //202 = classic DDG "slow down", 403 = blocked - both observed live.
            status @ (StatusCode::ACCEPTED
            | StatusCode::FORBIDDEN
            | StatusCode::TOO_MANY_REQUESTS) => Err(ArgosError::RateLimited {
                engine: ENGINE.into(),
                status: status.as_u16(),
            }),
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

    const FIXTURE: &str = include_str!("../../tests/fixtures/duckduckgo.html");

    #[test]
    fn parses_real_fixture_without_ads() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert!(
            results.len() >= 8,
            "expected the real page's organic results, got {}",
            results.len()
        );
        for result in &results {
            assert!(!result.url.is_empty());
            assert!(!result.title.is_empty());
            assert!(!result.url.contains("/y.js"), "ad leaked into results");
            assert_eq!(result.engine.as_deref(), Some("duckduckgo"));
        }
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
    fn unwraps_redirect_hrefs() {
        assert_eq!(
            unwrap_href("//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fx%3Fa%3D1&rut=abc"),
            "https://example.com/x?a=1"
        );
        assert_eq!(
            unwrap_href("https://direct.example/page"),
            "https://direct.example/page"
        );
        assert_eq!(unwrap_href("//example.com/abs"), "https://example.com/abs");
    }

    #[test]
    fn rejects_garbage_html_gracefully() {
        let results = parse_results("<html><body>captcha page</body></html>").expect("parses");
        assert!(results.is_empty());
    }
}
