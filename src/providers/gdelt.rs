//! GDELT DOC 2.0 adapter (`P1_VERTICAL`, news profile).
//!
//! GDELT indexes global news from the past three months and exposes a documented
//! JSON endpoint with no key. It is the only keyless world-news source in the
//! registry, which is why it gets its own `news` profile.
//!
//! Pacing matters: GDELT answers 429 above roughly one request every five
//! seconds, so [`pace`] enforces that floor locally instead of burning the
//! shared cooldown on every burst. Measured 2026-10-01: this host's IP is
//! already throttled even when paced, so the provider degrades gracefully and
//! reports the throttle honestly rather than stalling the search.

use std::time::Duration;

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{
    SearchProvider, api_client, build_result, map_api_status, pace, retry_after_of,
};

const ENGINE: &str = "gdelt";
const ENDPOINT: &str = "https://api.gdeltproject.org/api/v2/doc/doc";

/// GDELT's published floor for anonymous clients.
const MIN_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    articles: Vec<RawArticle>,
}

#[derive(Deserialize, Debug, Default)]
struct RawArticle {
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    domain: Option<String>,
    #[serde(default)]
    sourcecountry: Option<String>,
    #[serde(rename = "seendate", default)]
    seen_date: Option<String>,
    #[serde(default)]
    language: Option<String>,
}

/// GDELT returns `seendate` as the compact form `20260101T153000Z`.
fn format_seen_date(raw: &str) -> Option<String> {
    let digits = raw.split('T').next()?.trim();
    (digits.len() == 8).then(|| format!("{}-{}-{}", &digits[0..4], &digits[4..6], &digits[6..8]))
}

/// Parse a GDELT `artlist` payload into compact results (pure).
pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .articles
        .into_iter()
        .filter_map(|article| {
            let url = article.url.trim();
            let title = article.title.trim();
            if url.is_empty() || title.is_empty() {
                return None;
            }
            let mut snippet = String::new();
            if let Some(domain) = article
                .domain
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(&format!("{domain}. "));
            }
            if let Some(country) = article
                .sourcecountry
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
            {
                snippet.push_str(&format!("{country}. "));
            }
            if let Some(date) = article.seen_date.as_deref().and_then(format_seen_date) {
                snippet.push_str(&format!("{date}. "));
            }
            if let Some(language) = article
                .language
                .as_deref()
                .map(str::trim)
                .filter(|l| !l.is_empty())
            {
                snippet.push_str(&format!("[{language}] "));
            }
            Some(build_result(ENGINE, url, title, &snippet))
        })
        .collect())
}

/// GDELT provider over the shared public-API HTTP client.
pub struct GdeltProvider {
    config: Config,
}

impl GdeltProvider {
    /// Create the provider for the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for GdeltProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        // GDELT has no page parameter: it returns a single relevance window.
        let max_records = limit.clamp(1, 250);
        pace(ENGINE, MIN_INTERVAL).await;
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("query", query.to_string()),
                ("mode", "artlist".to_string()),
                ("format", "json".to_string()),
                ("maxrecords", max_records.to_string()),
                ("sort", "datedesc".to_string()),
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
            let retry =
                retry_after_of(&response).or((code == 429).then_some(MIN_INTERVAL.as_secs()));
            return Err(map_api_status(ENGINE, code, retry));
        }
        let payload = response
            .text()
            .await
            .map_err(|ex| ArgosError::Unreachable {
                origin: ENGINE.into(),
                cause: ex.to_string(),
            })?;
        let mut results = parse_results(&payload)?;
        if page > 1 {
            // Documented limitation rather than a silent repeat of page 1.
            results.clear();
        }
        results.truncate(limit);
        Ok(results)
    }

    async fn health(&self) -> bool {
        match api_client()
            .get(ENDPOINT)
            .query(&[
                ("query", "technology".to_string()),
                ("mode", "artlist".to_string()),
                ("format", "json".to_string()),
                ("maxrecords", "1".to_string()),
            ])
            .timeout(self.config.ping_timeout)
            .send()
            .await
        {
            // A throttled-but-answering endpoint is still reachable.
            Ok(response) => response.status().as_u16() != 503,
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "articles": [
        {
          "url": "https://example.com/news/story",
          "title": "Rust foundation announces something",
          "domain": "example.com",
          "sourcecountry": "United States",
          "seendate": "20260101T153000Z",
          "language": "English"
        },
        { "url": "", "title": "no url" },
        { "url": "https://example.org/x", "title": "no metadata" }
      ]
    }"#;

    #[test]
    fn formats_the_compact_seen_date() {
        assert_eq!(
            format_seen_date("20260101T153000Z").as_deref(),
            Some("2026-01-01")
        );
        assert!(format_seen_date("garbage").is_none());
        assert!(format_seen_date("").is_none());
    }

    #[test]
    fn keeps_usable_records_and_drops_empty_ones() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 2, "a record without a url is unusable");
        assert_eq!(results[0].url, "https://example.com/news/story");
        assert!(
            results[0]
                .snippet
                .starts_with("example.com. United States. 2026-01-01. [English]")
        );
        // A record with no metadata still produces a result, with an empty snippet.
        assert_eq!(results[1].title, "no metadata");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("rate limited").is_err());
        assert!(parse_results("{\"articles\": []}").is_ok_and(|r| r.is_empty()));
    }
}
