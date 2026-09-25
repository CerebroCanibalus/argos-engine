//! Search providers: keyless direct engines via browser impersonation.
//!
//! Measured from this machine (2026-09-24):
//! - plain reqwest (rustls AND schannel): Bing 302-redirects to its homepage,
//!   Brave returns 429, Mojeek serves a CAPTCHA page; DuckDuckGo works.
//! - primp impersonating Chrome153/Windows: **Bing, Brave and DuckDuckGo all
//!   answer 200 with real results**; Mojeek still CAPTCHA (IP/consent-based).
//!
//! Engine-specific blocking maps to [`ArgosError::RateLimited`] so the fanout
//! can fail over.

pub mod bing;
pub mod brave;
pub mod duckduckgo;
pub mod fanout;
pub mod searxng;

use std::sync::OnceLock;
use std::time::Duration;

use flojo_mcp::async_trait::async_trait;

use crate::config::Config;
use crate::error::ArgosError;
use crate::types::SearchResult;

const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);

fn build_client(timeout: Duration) -> primp::Client {
    primp::Client::builder()
        .impersonate(primp::Impersonate::ChromeV153)
        .impersonate_os(primp::ImpersonateOS::Windows)
        .timeout(timeout)
        .build()
        .expect("primp client builder")
}

/// Shared search client: Chrome153/Windows impersonation (TLS + HTTP/2 +
/// browser headers) with the configured request timeout. Built once; the
/// timeout comes from the environment of the first search (env is fixed per
/// process, which matches env-var semantics).
pub fn impersonated_client(config: &Config) -> primp::Client {
    static CLIENT: OnceLock<primp::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| build_client(config.request_timeout))
        .clone()
}

/// Shared short-timeout client for reachability probes (never hangs `status`).
pub fn impersonated_health_client() -> primp::Client {
    static CLIENT: OnceLock<primp::Client> = OnceLock::new();
    CLIENT.get_or_init(|| build_client(HEALTH_TIMEOUT)).clone()
}

/// Compact hostname for the `source` field (no scheme, no path, no `www.`).
/// Falls back to a manual split when the URL parser rejects the input.
pub(crate) fn compact_source(url: &str) -> String {
    let host = url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(|s| s.to_string()))
        .unwrap_or_else(|| {
            let stripped = url
                .trim_start_matches("https://")
                .trim_start_matches("http://");
            stripped.split('/').next().unwrap_or("").to_string()
        });
    let trimmed = host.trim_start_matches("www.");
    if trimmed.is_empty() {
        String::new()
    } else {
        crate::limits::truncate_source(trimmed)
    }
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
