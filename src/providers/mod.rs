//! Search providers: keyless direct engines by default, optional local SearXNG.
//!
//! Measured from this machine (2026-09-24): html.duckduckgo.com answers **200**
//! to plain reqwest/rustls with a browser UA (PowerShell's default UA gets 403);
//! search.brave.com returned 429 and www.mojeek.com served a CAPTCHA page on
//! the same run - they are NOT in the default set. All engine-specific blocking
//! maps to [`ArgosError::RateLimited`] so the fanout can fail over.

pub mod bing;
pub mod duckduckgo;
pub mod fanout;
pub mod searxng;

use std::sync::OnceLock;

use flojo_mcp::async_trait::async_trait;

use crate::error::ArgosError;
use crate::types::SearchResult;

/// Browser-grade User-Agent. Verified: with it, reqwest/rustls gets HTTP 200
/// from html.duckduckgo.com; without a browser UA clients get 403.
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// Shared, pooled client with browser-ish default headers for keyless engines.
pub fn keyless_client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::USER_AGENT,
                reqwest::header::HeaderValue::from_static(USER_AGENT),
            );
            headers.insert(
                reqwest::header::ACCEPT,
                reqwest::header::HeaderValue::from_static(
                    "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
                ),
            );
            headers.insert(
                reqwest::header::ACCEPT_LANGUAGE,
                reqwest::header::HeaderValue::from_static("en-US,en;q=0.9"),
            );
            reqwest::Client::builder()
                .default_headers(headers)
                .build()
                .expect("reqwest default client builder is infallible")
        })
        .clone()
}

/// A source of web results. Implementations must be cancellation-agnostic
/// and fast-failing (bounded by their own per-request timeouts).
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Run a paged search and return normalized, compact results.
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<SearchResult>, ArgosError>;

    /// Cheap reachability probe; `false` when the source is unreachable.
    async fn health(&self) -> bool;
}
