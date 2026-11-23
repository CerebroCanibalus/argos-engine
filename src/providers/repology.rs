//! Repology adapter (`code` profile).
//!
//! Public projects search API, no key. Returns project name, repo count and
//! a canonical repology.org URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "repology";
const ENDPOINT: &str = "https://repology.org/api/v1/projects";

#[derive(Deserialize, Debug, Default)]
struct RawProject {
    #[serde(default, rename = "repo")]
    repo: Option<String>,
    #[serde(default)]
    version: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    // Repology returns an object keyed by project name.
    let parsed: std::collections::BTreeMap<String, Vec<RawProject>> =
        flojo_mcp::serde_json::from_str(payload).map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    let mut results = Vec::new();
    for (name, entries) in parsed.into_iter() {
        if name.trim().is_empty() {
            continue;
        }
        let entry = entries.into_iter().next().unwrap_or_default();
        let url = format!("https://repology.org/project/{name}/versions");
        let mut snippet = String::new();
        if let Some(version) = entry.version.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            snippet.push_str(&format!("v{version}. "));
        }
        if let Some(repo) = entry.repo.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
            snippet.push_str(&format!("Repo: {repo}. "));
        }
        results.push(build_result(ENGINE, &url, &name, &snippet));
    }
    Ok(results)
}

pub struct RepologyProvider {
    config: Config,
}

impl RepologyProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for RepologyProvider {
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
                ("limit", per_page.to_string()),
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
        results.truncate(limit);
        Ok(results)
    }

    async fn health(&self) -> bool {
        match api_client()
            .get(ENDPOINT)
            .query(&[("search", "rust".to_string()), ("limit", "1".to_string())])
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
      "rust": [
        { "repo": "arch", "srcname": "rust", "version": "1.75.0", "visiblename": "rust" }
      ],
      "python": [
        { "repo": "debian", "version": "3.11.0" }
      ]
    }"#;

    #[test]
    fn parses_projects() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|r| r.title == "rust" && r.snippet.contains("v1.75.0.")));
        assert!(results.iter().any(|r| r.title == "python"));
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
