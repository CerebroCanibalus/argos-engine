//! Docker Hub adapter (`code` profile).
//!
//! Public search endpoint, no key. Returns repository name, description,
//! star count and a canonical hub.docker.com URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "dockerhub";
const ENDPOINT: &str = "https://hub.docker.com/v2/search/repositories";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    results: Vec<RawRepo>,
}

#[derive(Deserialize, Debug, Default)]
struct RawRepo {
    #[serde(default)]
    repo_name: Option<String>,
    #[serde(default)]
    repo_owner: Option<String>,
    #[serde(default)]
    short_description: Option<String>,
    #[serde(default)]
    star_count: Option<i64>,
    #[serde(default)]
    pull_count: Option<i64>,
    #[serde(default)]
    is_official: Option<bool>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .results
        .into_iter()
        .filter_map(|r| {
            let name = r
                .repo_name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())?;
            let owner = r
                .repo_owner
                .as_deref()
                .map(str::trim)
                .filter(|o| !o.is_empty());
            let full = match owner {
                Some(o) => format!("{o}/{name}"),
                None => name.to_string(),
            };
            let url = format!("https://hub.docker.com/r/{full}");
            let mut snippet = String::new();
            if let Some(desc) = r
                .short_description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(desc);
                snippet.push_str(". ");
            }
            if r.is_official == Some(true) {
                snippet.push_str("Official. ");
            }
            if let Some(stars) = r.star_count {
                snippet.push_str(&format!("{stars} stars. "));
            }
            if let Some(pulls) = r.pull_count {
                snippet.push_str(&format!("{pulls} pulls. "));
            }
            Some(build_result(ENGINE, &url, &full, &snippet))
        })
        .collect())
}

pub struct DockerHubProvider {
    config: Config,
}

impl DockerHubProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for DockerHubProvider {
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
                ("query", query.to_string()),
                ("page_size", per_page.to_string()),
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
            .query(&[
                ("query", "rust".to_string()),
                ("page_size", "1".to_string()),
            ])
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
      "results": [
        {
          "repo_name": "rust",
          "repo_owner": "library",
          "short_description": "Rust programming language",
          "star_count": 1000,
          "pull_count": 5000000,
          "is_official": true
        },
        { "repo_name": "" }
      ]
    }"#;

    #[test]
    fn parses_repos_and_skips_nameless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "library/rust");
        assert!(results[0].snippet.contains("Rust programming language."));
        assert!(results[0].snippet.contains("Official."));
        assert!(results[0].snippet.contains("1000 stars."));
        assert!(results[0].snippet.contains("5000000 pulls."));
        assert_eq!(results[0].url, "https://hub.docker.com/r/library/rust");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
