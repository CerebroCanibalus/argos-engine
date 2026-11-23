//! Conda adapter (`code` profile).
//!
//! Public Anaconda package search, no key. Returns package name, summary and
//! a canonical anaconda.org URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "conda";
const ENDPOINT: &str = "https://api.anaconda.org/search";

#[derive(Deserialize, Debug, Default)]
struct RawResult {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    owner: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: Vec<RawResult> = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .into_iter()
        .filter_map(|r| {
            let name = r.name.as_deref().map(str::trim).filter(|n| !n.is_empty())?;
            let owner = r.owner.as_deref().map(str::trim).filter(|o| !o.is_empty()).unwrap_or("");
            let full = if owner.is_empty() {
                name.to_string()
            } else {
                format!("{owner}/{name}")
            };
            let url = format!("https://anaconda.org/{full}");
            let mut snippet = String::new();
            if let Some(summary) = r.summary.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                snippet.push_str(summary);
                snippet.push_str(". ");
            }
            Some(build_result(ENGINE, &url, &full, &snippet))
        })
        .collect())
}

pub struct CondaProvider {
    config: Config,
}

impl CondaProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for CondaProvider {
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
                ("name", query.to_string()),
                ("page", page.to_string()),
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
            .query(&[("name", "numpy".to_string())])
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

    const FIXTURE: &str = r#"[
      {
        "_id": "1",
        "name": "numpy",
        "summary": "Fundamental package for array computing",
        "owner": "conda-forge"
      },
      { "name": "" }
    ]"#;

    #[test]
    fn parses_packages_and_skips_nameless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "conda-forge/numpy");
        assert!(results[0].snippet.contains("Fundamental package for array computing."));
        assert_eq!(results[0].url, "https://anaconda.org/conda-forge/numpy");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
