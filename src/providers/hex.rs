//! Hex.pm adapter (`code` profile).
//!
//! Public packages search endpoint, no key. Returns package name, latest
//! version, description and a canonical hex.pm URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "hex";
const ENDPOINT: &str = "https://hex.pm/api/packages";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    packages: Option<Vec<RawPackage>>,
    // Some deployments return a top-level array instead.
}

#[derive(Deserialize, Debug, Default)]
struct RawPackage {
    #[serde(default)]
    name: String,
    #[serde(default)]
    meta: Option<RawMeta>,
    #[serde(default)]
    latest_version: Option<String>,
    #[serde(default, rename = "latest_stable_version")]
    latest_stable_version: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct RawMeta {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    licenses: Option<Vec<String>>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    // Accept either {"packages":[...]} or a bare array.
    let packages: Vec<RawPackage> =
        if let Ok(wrapped) = flojo_mcp::serde_json::from_str::<RawResponse>(payload) {
            wrapped.packages.unwrap_or_default()
        } else if let Ok(arr) = flojo_mcp::serde_json::from_str::<Vec<RawPackage>>(payload) {
            arr
        } else {
            return Err(ArgosError::Decode("unexpected hex payload".into()));
        };
    Ok(packages
        .into_iter()
        .filter_map(|pkg| {
            let name = pkg.name.trim();
            if name.is_empty() {
                return None;
            }
            let url = format!("https://hex.pm/packages/{name}");
            let version = pkg
                .latest_stable_version
                .as_deref()
                .or(pkg.latest_version.as_deref())
                .map(str::trim)
                .filter(|v| !v.is_empty());
            let mut snippet = String::new();
            if let Some(version) = version {
                snippet.push_str(&format!("v{version}. "));
            }
            if let Some(meta) = pkg.meta.as_ref() {
                if let Some(description) = meta
                    .description
                    .as_deref()
                    .map(str::trim)
                    .filter(|d| !d.is_empty())
                {
                    snippet.push_str(description);
                    snippet.push_str(". ");
                }
                if let Some(licenses) = meta.licenses.as_ref().filter(|l| !l.is_empty()) {
                    snippet.push_str(&format!("Licenses: {}. ", licenses.join(", ")));
                }
            }
            Some(build_result(ENGINE, &url, name, &snippet))
        })
        .collect())
}

pub struct HexProvider {
    config: Config,
}

impl HexProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for HexProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 100);
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("search", query.to_string()),
                ("sort", "recent_downloads".to_string()),
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
            let skip = ((page.saturating_sub(1)) * per_page).min(results.len());
            results.drain(..skip);
        }
        results.truncate(limit);
        Ok(results)
    }

    async fn health(&self) -> bool {
        match api_client()
            .get(ENDPOINT)
            .query(&[("search", "json".to_string())])
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
      "packages": [
        {
          "name": "jason",
          "latest_stable_version": "1.4.0",
          "meta": {
            "description": "A blazing fast JSON parser and generator",
            "licenses": ["Apache-2.0"]
          }
        },
        { "name": "" }
      ]
    }"#;

    #[test]
    fn parses_packages_and_skips_nameless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "jason");
        assert!(results[0].snippet.contains("v1.4.0."));
        assert!(
            results[0]
                .snippet
                .contains("A blazing fast JSON parser and generator.")
        );
        assert!(results[0].snippet.contains("Licenses: Apache-2.0."));
        assert_eq!(results[0].url, "https://hex.pm/packages/jason");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
