//! Maven Central adapter (`code` profile).
//!
//! Solr search endpoint, no key. Matches groupId/artifactId and returns a
//! canonical Maven Central artifact URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "maven";
const ENDPOINT: &str = "https://search.maven.org/solrsearch/select";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    response: RawInner,
}

#[derive(Deserialize, Debug, Default)]
struct RawInner {
    #[serde(default)]
    docs: Vec<RawDoc>,
}

#[derive(Deserialize, Debug, Default)]
struct RawDoc {
    #[serde(default, rename = "g")]
    group_id: Option<String>,
    #[serde(default, rename = "a")]
    artifact_id: Option<String>,
    #[serde(default, rename = "latestVersion")]
    latest_version: Option<String>,
    #[serde(default, rename = "p")]
    packaging: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .response
        .docs
        .into_iter()
        .filter_map(|doc| {
            let group = doc
                .group_id
                .as_deref()
                .map(str::trim)
                .filter(|g| !g.is_empty())?;
            let artifact = doc
                .artifact_id
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())?;
            let url = format!("https://search.maven.org/artifact/{group}/{artifact}");
            let mut snippet = String::new();
            if let Some(version) = doc
                .latest_version
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                snippet.push_str(&format!("Latest {version}. "));
            }
            if let Some(packaging) = doc
                .packaging
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
            {
                snippet.push_str(&format!("Packaging: {packaging}. "));
            }
            if let Some(timestamp) = doc.timestamp {
                snippet.push_str(&format!("Updated {timestamp}. "));
            }
            Some(build_result(
                ENGINE,
                &url,
                &format!("{group}:{artifact}"),
                &snippet,
            ))
        })
        .collect())
}

pub struct MavenProvider {
    config: Config,
}

impl MavenProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for MavenProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 100);
        let offset = (page.saturating_sub(1)) * per_page;
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("q", query.to_string()),
                ("rows", per_page.to_string()),
                ("start", offset.to_string()),
                ("wt", "json".to_string()),
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
                ("q", "rust".to_string()),
                ("rows", "1".to_string()),
                ("wt", "json".to_string()),
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
      "response": {
        "docs": [
          {
            "g": "org.apache.commons",
            "a": "commons-lang3",
            "latestVersion": "3.14.0",
            "p": "jar",
            "timestamp": 1700000000
          },
          { "g": "", "a": "no-group" }
        ]
      }
    }"#;

    #[test]
    fn parses_artifacts_and_skips_groupless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "org.apache.commons:commons-lang3");
        assert!(results[0].snippet.contains("Latest 3.14.0."));
        assert!(results[0].snippet.contains("Packaging: jar."));
        assert_eq!(
            results[0].url,
            "https://search.maven.org/artifact/org.apache.commons/commons-lang3"
        );
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
