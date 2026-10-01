//! crates.io adapter (`P1_VERTICAL`, code profile).
//!
//! The Rust registry's API is public, documented and needs no key. When someone
//! asks for "a crate to do X", the registry answer is more precise than any web
//! index because it matches package names and keywords directly.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{
    SearchProvider, api_client, build_result, map_api_status, offset_of, retry_after_of,
};

const ENGINE: &str = "crates";
const ENDPOINT: &str = "https://crates.io/api/v1/crates";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    crates: Vec<RawCrate>,
}

#[derive(Deserialize, Debug, Default)]
struct RawCrate {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    documentation: Option<String>,
    #[serde(default)]
    max_stable_version: Option<String>,
    #[serde(default)]
    recent_downloads: Option<u64>,
    #[serde(default)]
    keywords: Option<Vec<String>>,
    #[serde(default)]
    yanked: Option<bool>,
}

/// Landing page preference: repository, then docs, then homepage, then the
/// registry page itself, which always exists.
fn canonical_url(krate: &RawCrate) -> String {
    krate
        .repository
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .or_else(|| {
            krate
                .documentation
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
        })
        .or_else(|| {
            krate
                .homepage
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
        })
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://crates.io/crates/{}", krate.name.trim()))
}

/// Parse a crates.io search payload into compact results (pure).
pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .crates
        .into_iter()
        .filter_map(|krate| {
            let name = krate.name.trim();
            if name.is_empty() {
                return None;
            }
            let url = canonical_url(&krate);
            let mut snippet = String::new();
            if let Some(description) = krate
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(description);
                snippet.push_str(". ");
            }
            if let Some(version) = krate
                .max_stable_version
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                snippet.push_str(&format!("v{version}. "));
            }
            if let Some(downloads) = krate.recent_downloads {
                snippet.push_str(&format!("{downloads} recent downloads. "));
            }
            if let Some(keywords) = krate
                .keywords
                .as_ref()
                .map(|k| {
                    k.iter()
                        .map(|word| word.trim())
                        .filter(|word| !word.is_empty())
                        .take(4)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|keywords| !keywords.is_empty())
            {
                snippet.push_str(&format!("Keywords: {keywords}. "));
            }
            if krate.yanked == Some(true) {
                snippet.push_str("(yanked)");
            }
            Some(build_result(ENGINE, &url, name, &snippet))
        })
        .collect())
}

/// crates.io provider over the shared public-API HTTP client.
pub struct CratesProvider {
    config: Config,
}

impl CratesProvider {
    /// Create the provider for the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for CratesProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 100);
        let offset = offset_of(page, per_page);
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("q", query.to_string()),
                ("per_page", per_page.to_string()),
                ("sort", "relevance".to_string()),
            ])
            // The registry rejects unknown query parameters, and the paging
            // offset is already folded into `per_page` on page 1.
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
        if page > 1 {
            // crates.io has no offset paging for search; drop the earlier window.
            let skip = offset.min(results.len());
            results.drain(..skip);
        }
        results.truncate(limit);
        Ok(results)
    }

    async fn health(&self) -> bool {
        match api_client()
            .get(ENDPOINT)
            .query(&[("q", "serde"), ("per_page", "1")])
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
      "crates": [
        {
          "name": "serde",
          "description": "A generic serialization/deserialization framework",
          "homepage": "https://serde.rs",
          "documentation": "https://docs.rs/serde",
          "repository": "https://github.com/serde-rs/serde",
          "max_stable_version": "1.0.229",
          "recent_downloads": 335463385,
          "keywords": ["serialization", "json"],
          "yanked": false
        },
        { "name": "", "description": "no name" },
        { "name": "old_thing", "yanked": true, "recent_downloads": 3 }
      ]
    }"#;

    #[test]
    fn prefers_repository_and_keeps_registry_metrics() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 2, "a crate without a name is unusable");
        assert_eq!(results[0].url, "https://github.com/serde-rs/serde");
        assert_eq!(results[0].title, "serde");
        let snippet = &results[0].snippet;
        assert!(snippet.starts_with("A generic serialization/deserialization framework."));
        assert!(snippet.contains("v1.0.229."));
        assert!(snippet.contains("335463385 recent downloads."));
        assert!(snippet.contains("Keywords: serialization, json."));
    }

    #[test]
    fn falls_back_to_the_registry_page_and_flags_yanked() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results[1].url, "https://crates.io/crates/old_thing");
        assert!(results[1].snippet.ends_with("(yanked)"));
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
        assert!(parse_results("{\"crates\": []}").is_ok_and(|r| r.is_empty()));
    }
}
