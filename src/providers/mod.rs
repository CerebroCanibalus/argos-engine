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

pub mod arxiv;
pub mod bing;
pub mod brave;
pub mod crates;
pub mod crossref;
pub mod doaj;
pub mod duckduckgo;
pub mod europepmc;
pub mod fanout;
pub mod gdelt;
pub mod github;
pub mod npm;
pub mod openalex;
pub mod packagist;
pub mod pubmed;
pub mod searxng;
pub mod semantic_scholar;
pub mod wikimedia;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use flojo_mcp::async_trait::async_trait;

use crate::config::Config;
use crate::error::ArgosError;
use crate::types::SearchResult;

const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);

/// Identification sent by public-API adapters. Bibliographic services ask for
/// a contactable client, and a real agent name is the polite minimum.
pub const ARGO_USER_AGENT: &str = concat!(
    env!("CARGO_PKG_NAME"),
    "/",
    env!("CARGO_PKG_VERSION"),
    " (keyless metasearch MCP)"
);

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

/// Shared client for documented public JSON/XML APIs.
///
/// Browser impersonation is deliberately *not* used here: these endpoints are
/// public APIs that want an identified agent, not a spoofed browser. Keeping a
/// separate, reuse-friendly client avoids a cold connection per tool call.
pub fn api_client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .user_agent(ARGO_USER_AGENT)
                .build()
                .expect("reqwest default client builder is infallible")
        })
        .clone()
}

/// Parse a standard numeric `Retry-After` value.
///
/// HTTP-date forms are intentionally ignored: the caller then falls back to the
/// conservative local cooldown, which is safe because it can only over-wait.
pub fn parse_retry_after(raw: &str) -> Option<u64> {
    raw.trim().parse::<u64>().ok()
}

/// One-based page to a zero-based record offset.
pub(crate) fn offset_of(page: usize, per_page: usize) -> usize {
    page.saturating_sub(1).saturating_mul(per_page)
}

/// Collapse the punctuation accidents that assembling snippets produces.
///
/// Upstream free-text fields (author strings, journal titles, publishers) very
/// often end in their own full stop, so joining them with `". "` produces
/// "Kimei EH, Nyambo DG.. Front Artif Intell.". Deduping the repeated mark here
/// fixes every adapter at once, because this is the single funnel they all pass
/// through.
fn tidy_punctuation(joined: &str) -> String {
    let mut out = joined.replace(" .", ".").replace(" ,", ",");
    while out.contains("..") || out.contains(",,") {
        out = out.replace("..", ".").replace(",,", ",");
    }
    out
}

/// Whether the upstream index already ranks by relevance to the query.
///
/// The lexical gate in `quality::is_relevant` exists to defend against HTML
/// search pages: they drift, mix intents and can answer a completely different
/// question while returning HTTP 200. A curated API (a registry, a
/// bibliographic index, a news wire) *is* the relevance signal, so re-filtering
/// its output only throws away good results - measured: crates.io dropped
/// matching crates because their repository URL came back instead of the
/// registry page.
///
/// Domain scoping still applies everywhere; only the lexical filter is skipped.
pub(crate) fn provider_trusts_relevance(name: &str) -> bool {
    !matches!(name, "duckduckgo" | "bing" | "brave" | "searxng")
}

/// Assemble a compact, truncated [`SearchResult`] from collected parts.
///
/// Every adapter funnels through here so the token budgets (`title`/`snippet`)
/// and the `source`/`engine` normalisation cannot drift between providers.
pub(crate) fn build_result(engine: &str, url: &str, title: &str, snippet: &str) -> SearchResult {
    let collapsed = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
    SearchResult {
        source: compact_source(url),
        url: url.to_string(),
        title: flojo_mcp::truncate_string(title.trim(), crate::limits::TITLE_MAX_CHARS).0,
        snippet: flojo_mcp::truncate_string(
            &tidy_punctuation(collapsed.trim()),
            crate::limits::SNIPPET_MAX_CHARS,
        )
        .0,
        engine: Some(engine.to_string()),
    }
}

/// Map a non-success status from a public API onto Argos' typed errors.
///
/// The distinction matters to the agent: 403/429 means "this IP is throttled,
/// try later or elsewhere", while any other code means the provider is broken.
pub(crate) fn map_api_status(
    engine: &str,
    status: u16,
    retry_after_secs: Option<u64>,
) -> ArgosError {
    if status == 429 || status == 403 {
        ArgosError::RateLimited {
            engine: engine.to_string(),
            status,
            retry_after_secs,
        }
    } else {
        ArgosError::Unreachable {
            origin: engine.to_string(),
            cause: format!("HTTP {status}"),
        }
    }
}

/// Read `Retry-After` from a response's headers.
pub(crate) fn retry_after_of(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after)
}

/// Enforce a minimum interval between consecutive requests per upstream.
///
/// Some public APIs publish a hard floor (GDELT asks for one request every five
/// seconds). Racing past it only earns a 429, so the wait happens here instead
/// of being paid for by the shared cooldown afterwards.
pub(crate) async fn pace(key: &str, minimum: Duration) {
    static LAST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    let last = LAST.get_or_init(|| Mutex::new(HashMap::new()));
    let now = Instant::now();
    let wait = {
        let last = last.lock().expect("upstream pacing lock");
        match last.get(key) {
            Some(previous) => minimum.saturating_sub(now.duration_since(*previous)),
            None => Duration::ZERO,
        }
        .max(Duration::ZERO)
    };
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
    last.lock()
        .expect("upstream pacing lock")
        .insert(key.to_string(), Instant::now());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidy_punctuation_removes_duplicated_marks() {
        assert_eq!(
            tidy_punctuation("Kimei EH, Nyambo DG.. Front Artif Intell. 2026."),
            "Kimei EH, Nyambo DG. Front Artif Intell. 2026."
        );
        assert_eq!(tidy_punctuation("Title. . Journal"), "Title. Journal");
        assert_eq!(tidy_punctuation("a,, b"), "a, b");
        // Real version numbers must survive untouched.
        assert_eq!(
            tidy_punctuation("v1.0.229. 42 downloads"),
            "v1.0.229. 42 downloads"
        );
        assert_eq!(tidy_punctuation("no problems here"), "no problems here");
    }

    #[test]
    fn build_result_applies_budgets_and_engine_tag() {
        let result = build_result(
            "test",
            "https://doi.org/10.1000/x",
            "  A Title  ",
            "Author, One.. Two.   Third   ",
        );
        assert_eq!(result.source, "doi.org");
        assert_eq!(result.title, "A Title");
        assert_eq!(result.snippet, "Author, One. Two. Third");
        assert_eq!(result.engine.as_deref(), Some("test"));
    }

    #[test]
    fn curated_apis_skip_the_lexical_drift_guard() {
        // HTML pages drift, curated indexes do not.
        for html in ["duckduckgo", "bing", "brave", "searxng"] {
            assert!(
                !provider_trusts_relevance(html),
                "{html} must keep the lexical gate"
            );
        }
        for api in [
            "openalex",
            "crossref",
            "arxiv",
            "github",
            "crates",
            "npm",
            "packagist",
            "wikimedia",
            "gdelt",
            "europe_pmc",
            "pubmed",
            "doaj",
            "semantic_scholar",
        ] {
            assert!(
                provider_trusts_relevance(api),
                "{api} is already relevance-ranked upstream"
            );
        }
    }

    #[test]
    fn offset_and_status_helpers() {
        assert_eq!(offset_of(1, 10), 0);
        assert_eq!(offset_of(4, 25), 75);
        assert!(matches!(
            map_api_status("svc", 429, Some(30)),
            ArgosError::RateLimited {
                status: 429,
                retry_after_secs: Some(30),
                ..
            }
        ));
        assert!(matches!(
            map_api_status("svc", 500, None),
            ArgosError::Unreachable { .. }
        ));
        assert_eq!(parse_retry_after("  12 "), Some(12));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
    }

    #[tokio::test]
    async fn pace_waits_for_the_previous_call() {
        // Two calls to one upstream must be separated by the floor.
        let start = Instant::now();
        pace("test-upstream", Duration::from_millis(150)).await;
        pace("test-upstream", Duration::from_millis(150)).await;
        assert!(
            start.elapsed() >= Duration::from_millis(100),
            "the second call must respect the interval"
        );
    }
}
