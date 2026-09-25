//! Bing html endpoint provider (keyless, no VM).

use base64::Engine;
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

const ENGINE: &str = "bing";
const ENDPOINT: &str = "https://www.bing.com/search";
const HOST: &str = "www.bing.com";
const EDGE_COOKIES: &str = "_EDGE_CD=m=en-us&u=en-us; _EDGE_S=mkt=en-us&ui=en-us";

/// Bing html provider: `li.b_algo` blocks with `u=a1<base64url>` wrapped links.
pub struct BingProvider {
    config: Config,
}

impl BingProvider {
    /// Create the provider for the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

/// Decode Bing's `ck/a?u=a1<base64url>` wrapped URL to the original target.
fn unwrap_ck(href: &str) -> Option<String> {
    if !href.contains("bing.com/ck/a?") {
        return None;
    }
    let query = href.split('?').nth(1)?;
    let encoded = query.split('&').find_map(|part| part.strip_prefix("u="))?;
    if encoded.len() <= 2 {
        return None;
    }
    let b64 = &encoded[2..];
    let padded = format!("{b64}{}", "=".repeat((4 - b64.len() % 4) % 4));
    let bytes = base64::engine::general_purpose::URL_SAFE
        .decode(padded)
        .ok()?;
    String::from_utf8(bytes).ok()
}

/// Parse a Bing results page (pure - fixture-testable, no I/O).
pub fn parse_results(html: &str) -> Result<Vec<SearchResult>, ArgosError> {
    use scraper::{Html, Selector};

    let document = Html::parse_document(html);
    let block = Selector::parse("li.b_algo").expect("literal selector");
    let link = Selector::parse("h2 a").expect("literal selector");
    let snippet_sel = Selector::parse("p").expect("literal selector");

    let mut results = Vec::new();
    for item in document.select(&block) {
        let Some(anchor) = item.select(&link).next() else {
            continue;
        };
        let Some(href) = anchor.value().attr("href") else {
            continue;
        };
        // Ads are not organic results.
        if href.contains("bing.com/aclick?") {
            continue;
        }
        let url = unwrap_ck(href).unwrap_or_else(|| href.to_string());
        if !url.starts_with("http") {
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
            source: compact_source(&url),
            url,
            title: truncate_string(&title, TITLE_MAX_CHARS).0,
            snippet: truncate_string(&snippet, SNIPPET_MAX_CHARS).0,
            engine: Some(ENGINE.into()),
        });
    }
    Ok(results)
}

#[async_trait]
impl SearchProvider for BingProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<SearchResult>, ArgosError> {
        // Same shape ddgs ships: q/pq/cc plus the `first` offset for deeper pages.
        let mut params: Vec<(&str, String)> = vec![
            ("q", query.to_string()),
            ("pq", query.to_string()),
            ("cc", "en".to_string()),
        ];
        if page > 1 {
            let offset = (page - 1) * 10;
            params.push(("first", offset.to_string()));
        }

        let response = impersonated_client(&self.config)
            .get(ENDPOINT)
            .header("Cookie", EDGE_COOKIES)
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
            //429/403 observed under automated load - classic anti-bot answers.
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

    const FIXTURE: &str = include_str!("../../tests/fixtures/bing.html");

    #[test]
    fn parses_real_fixture_without_ads() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert!(
            results.len() >= 8,
            "expected the real page's organic results, got {}",
            results.len()
        );
        for result in &results {
            assert!(result.url.starts_with("http"));
            assert!(!result.title.is_empty());
            assert!(!result.url.contains("bing.com/aclick?"), "ad leaked");
            assert_eq!(result.engine.as_deref(), Some("bing"));
        }
    }

    #[test]
    fn unwraps_ck_wrapped_urls() {
        // "https://example.com" in base64url, prefixed the way Bing does (u=a1...).
        let target = "https://example.com";
        let b64 = base64::engine::general_purpose::URL_SAFE.encode(target);
        let href = format!("https://www.bing.com/ck/a?u=a1{b64}&ntb=1");
        assert_eq!(unwrap_ck(&href).as_deref(), Some(target));
        assert_eq!(
            unwrap_ck("https://example.org/plain"),
            None,
            "plain URLs are not wrapped"
        );
    }

    #[test]
    fn truncates_to_token_budget() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        for result in &results {
            assert!(result.snippet.chars().count() <= SNIPPET_MAX_CHARS + 3);
        }
    }

    #[test]
    fn rejects_garbage_html_gracefully() {
        let results = parse_results("<html><body>challenge</body></html>").expect("parses");
        assert!(results.is_empty());
    }
}
