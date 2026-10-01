//! GitHub repository search adapter (`P1_VERTICAL`, code profile).
//!
//! Searching for a library almost always means searching for a repository, so
//! GitHub is the anchor of the `code` profile. The unauthenticated Search API
//! allows 60 requests per hour per IP, which is why the adapter is registered
//! with the highest priority of the code family (its quota is the scarcest) and
//! why an optional token is supported for users who want more.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{
    SearchProvider, api_client, build_result, map_api_status, offset_of, retry_after_of,
};

const ENGINE: &str = "github";
const ENDPOINT: &str = "https://api.github.com/search/repositories";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    items: Vec<RawRepo>,
}

#[derive(Deserialize, Debug, Default)]
struct RawRepo {
    #[serde(default)]
    full_name: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    stargazers_count: Option<u32>,
    #[serde(default)]
    forks_count: Option<u32>,
    #[serde(default)]
    topics: Vec<String>,
    #[serde(default)]
    license: Option<License>,
    #[serde(default)]
    archived: Option<bool>,
}

#[derive(Deserialize, Debug)]
struct License {
    #[serde(default)]
    spdx_id: Option<String>,
}

/// Parse a GitHub repository search payload into compact results (pure).
pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .items
        .into_iter()
        .filter_map(|repo| {
            let url = repo
                .html_url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())?
                .to_string();
            let name = repo
                .full_name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .unwrap_or_default()
                .to_string();
            if name.is_empty() {
                return None;
            }
            let mut snippet = String::new();
            if let Some(description) = repo
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(description);
                snippet.push_str(". ");
            }
            if let Some(language) = repo
                .language
                .as_deref()
                .map(str::trim)
                .filter(|l| !l.is_empty())
            {
                snippet.push_str(&format!("{language}. "));
            }
            if let Some(stars) = repo.stargazers_count {
                snippet.push_str(&format!("{stars} stars"));
                if let Some(forks) = repo.forks_count {
                    snippet.push_str(&format!(", {forks} forks"));
                }
                snippet.push_str(". ");
            }
            if let Some(license) = repo
                .license
                .as_ref()
                .and_then(|l| l.spdx_id.as_deref())
                .map(str::trim)
                .filter(|l| !l.is_empty() && *l != "NOASSERTION")
            {
                snippet.push_str(&format!("{license}. "));
            }
            if !repo.topics.is_empty() {
                let topics = repo
                    .topics
                    .iter()
                    .map(|t| t.trim())
                    .filter(|t| !t.is_empty())
                    .take(4)
                    .collect::<Vec<_>>()
                    .join(", ");
                if !topics.is_empty() {
                    snippet.push_str(&format!("Topics: {topics}."));
                }
            }
            // Archived repositories are usually not what a searcher wants.
            if repo.archived == Some(true) {
                snippet.push_str(" (archived)");
            }
            Some(build_result(ENGINE, &url, &name, &snippet))
        })
        .collect())
}

/// GitHub provider over the shared public-API HTTP client.
pub struct GitHubProvider {
    config: Config,
}

impl GitHubProvider {
    /// Create the provider for the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for GitHubProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 100);
        let offset = offset_of(page, per_page);
        let params = vec![
            ("q", query.to_string()),
            ("per_page", per_page.to_string()),
            ("page", (offset / per_page + 1).to_string()),
            // Archived mirrors and forks are noise for library discovery.
            ("sort", "stars".to_string()),
        ];
        let mut request = api_client()
            .get(ENDPOINT)
            .query(&params)
            .timeout(self.config.request_timeout);
        if let Some(token) = &self.config.github_token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = request.send().await.map_err(|ex| ArgosError::Unreachable {
            origin: ENGINE.into(),
            cause: ex.to_string(),
        })?;

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            // GitHub's Search API answers 403 when the hourly quota is spent.
            let retry = retry_after_of(&response).or(match code {
                403 => Some(600),
                422 => Some(60),
                _ => None,
            });
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
        results.truncate(limit);
        Ok(results)
    }

    async fn health(&self) -> bool {
        match api_client()
            .get(ENDPOINT)
            .query(&[("q", "rust"), ("per_page", "1")])
            .timeout(self.config.ping_timeout)
            .send()
            .await
        {
            // An exhausted quota still means the API is alive and answering.
            Ok(response) => response.status().as_u16() != 503,
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "total_count": 319,
      "incomplete_results": false,
      "items": [
        {
          "full_name": "tokio-rs/tokio",
          "html_url": "https://github.com/tokio-rs/tokio",
          "description": "A runtime for writing reliable asynchronous applications",
          "language": "Rust",
          "stargazers_count": 27000,
          "forks_count": 2800,
          "topics": ["runtime", "async", "futures"],
          "license": {"spdx_id": "MIT"},
          "archived": false
        },
        {
          "full_name": "old/archived-thing",
          "html_url": "https://github.com/old/archived-thing",
          "language": null,
          "archived": true,
          "license": {"spdx_id": "NOASSERTION"}
        },
        { "description": "no name or url" }
      ]
    }"#;

    #[test]
    fn parses_repositories_and_keeps_metrics() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 2, "a repo without a name is unusable");
        assert_eq!(results[0].title, "tokio-rs/tokio");
        assert_eq!(results[0].url, "https://github.com/tokio-rs/tokio");
        let snippet = &results[0].snippet;
        assert!(snippet.starts_with("A runtime for writing reliable asynchronous applications."));
        assert!(snippet.contains("Rust. 27000 stars, 2800 forks. MIT."));
        assert!(snippet.ends_with("Topics: runtime, async, futures."));
    }

    #[test]
    fn flags_archived_and_drops_unasserted_license() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert!(results[1].snippet.ends_with("(archived)"));
        assert!(
            !results[1].snippet.contains("NOASSERTION"),
            "NOASSERTION is noise for the agent: {}",
            results[1].snippet
        );
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("<html/>").is_err());
        assert!(parse_results("{\"items\": []}").is_ok_and(|r| r.is_empty()));
        assert_eq!(offset_of(2, 10), 10);
    }
}
