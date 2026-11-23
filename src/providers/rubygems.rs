//! RubyGems adapter (`code` profile).
//!
//! Public search endpoint, no key. Returns gem name, version, info and
//! downloads with a canonical rubygems.org URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "rubygems";
const ENDPOINT: &str = "https://rubygems.org/api/v1/search.json";

#[derive(Deserialize, Debug, Default)]
struct RawGem {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    info: Option<String>,
    #[serde(default)]
    downloads: Option<u64>,
    #[serde(default)]
    homepage_uri: Option<String>,
    #[serde(default)]
    project_uri: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    // The endpoint returns a top-level array.
    let parsed: Vec<RawGem> = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .into_iter()
        .filter_map(|gem| {
            let name = gem.name.trim();
            if name.is_empty() {
                return None;
            }
            let url = gem
                .project_uri
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    gem.homepage_uri
                        .as_deref()
                        .map(str::trim)
                        .filter(|u| !u.is_empty())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| format!("https://rubygems.org/gems/{name}"));
            let mut snippet = String::new();
            if let Some(version) = gem
                .version
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                snippet.push_str(&format!("v{version}. "));
            }
            if let Some(info) = gem.info.as_deref().map(str::trim).filter(|i| !i.is_empty()) {
                snippet.push_str(info);
                snippet.push_str(". ");
            }
            if let Some(downloads) = gem.downloads {
                snippet.push_str(&format!("{downloads} downloads. "));
            }
            Some(build_result(ENGINE, &url, name, &snippet))
        })
        .collect())
}

pub struct RubygemsProvider {
    config: Config,
}

impl RubygemsProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for RubygemsProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 100);
        let response = api_client()
            .get(ENDPOINT)
            .query(&[("query", query.to_string())])
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
        // RubyGems has no paging in this endpoint; trim locally on later pages.
        if page > 1 {
            let skip = (page.saturating_sub(1) * per_page).min(results.len());
            results.drain(..skip);
        }
        results.truncate(limit);
        Ok(results)
    }

    async fn health(&self) -> bool {
        match api_client()
            .get(ENDPOINT)
            .query(&[("query", "json".to_string())])
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
        "name": "rails",
        "version": "7.1.0",
        "info": "Full-stack web application framework",
        "downloads": 400000000,
        "project_uri": "https://rubygems.org/gems/rails",
        "homepage_uri": "https://rubyonrails.org"
      },
      { "name": "", "version": "1.0" }
    ]"#;

    #[test]
    fn parses_gems_and_skips_nameless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "rails");
        assert!(results[0].snippet.contains("v7.1.0."));
        assert!(
            results[0]
                .snippet
                .contains("Full-stack web application framework.")
        );
        assert!(results[0].snippet.contains("400000000 downloads."));
        assert_eq!(results[0].url, "https://rubygems.org/gems/rails");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
