//! Packagist (PHP/Composer) search adapter (`P1_VERTICAL`, code profile).
//!
//! Packagist exposes a public JSON search endpoint. Together with crates.io and
//! npm it covers the three ecosystems where "find me a library" is the actual
//! question, without letting a web engine rank tutorials above packages.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{
    SearchProvider, api_client, build_result, map_api_status, offset_of, retry_after_of,
};

const ENGINE: &str = "packagist";
const ENDPOINT: &str = "https://packagist.org/search.json";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    results: Vec<RawPackage>,
}

#[derive(Deserialize, Debug, Default)]
struct RawPackage {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    downloads: Option<u64>,
    #[serde(default)]
    favers: Option<u32>,
}

/// Parse a Packagist search payload into compact results (pure).
pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .results
        .into_iter()
        .filter_map(|package| {
            let name = package.name.trim();
            if name.is_empty() {
                return None;
            }
            let url = package
                .repository
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .or_else(|| {
                    package
                        .url
                        .as_deref()
                        .map(str::trim)
                        .filter(|u| !u.is_empty())
                })
                .map(str::to_string)
                .unwrap_or_else(|| format!("https://packagist.org/packages/{name}"));
            let mut snippet = String::new();
            if let Some(description) = package
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(description);
                snippet.push_str(". ");
            }
            if let Some(downloads) = package.downloads {
                snippet.push_str(&format!("{downloads} downloads. "));
            }
            if let Some(favers) = package.favers {
                snippet.push_str(&format!("{favers} GitHub stars. "));
            }
            Some(build_result(ENGINE, &url, name, &snippet))
        })
        .collect())
}

/// Packagist provider over the shared public-API HTTP client.
pub struct PackagistProvider {
    config: Config,
}

impl PackagistProvider {
    /// Create the provider for the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for PackagistProvider {
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
            .query(&[("q", query.to_string()), ("per_page", per_page.to_string())])
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
        // Packagist search returns a single ranked window; deeper pages are
        // dropped rather than pretending the same window is page N.
        if page > 1 {
            results.drain(..offset.min(results.len()));
        }
        results.truncate(limit);
        Ok(results)
    }

    async fn health(&self) -> bool {
        match api_client()
            .get(ENDPOINT)
            .query(&[("q", "laravel"), ("per_page", "1")])
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
      "total": 200,
      "results": [
        {
          "name": "laravel/framework",
          "description": "The Laravel Framework.",
          "url": "https://packagist.org/packages/laravel/framework",
          "repository": "https://github.com/laravel/framework",
          "downloads": 587676934,
          "favers": 35432
        },
        { "name": "only-url", "url": "https://packagist.org/packages/only-url" },
        { "description": "no name" }
      ]
    }"#;

    #[test]
    fn prefers_repository_and_keeps_popularity() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 2, "a package without a name is unusable");
        assert_eq!(results[0].url, "https://github.com/laravel/framework");
        assert_eq!(results[0].title, "laravel/framework");
        let snippet = &results[0].snippet;
        assert!(snippet.starts_with("The Laravel Framework."));
        assert!(snippet.contains("587676934 downloads."));
        assert!(snippet.contains("35432 GitHub stars."));
    }

    #[test]
    fn falls_back_to_the_registry_page() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results[1].url, "https://packagist.org/packages/only-url");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
        assert!(parse_results("{\"results\": []}").is_ok_and(|r| r.is_empty()));
    }
}
