//! Arch User Repository adapter (`code` profile).
//!
//! Public RPC v5 search, no key. Returns package name, description, votes
//! and a canonical aur.archlinux.org URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "aur";
const ENDPOINT: &str = "https://aur.archlinux.org/rpc/v5/search";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    results: Vec<RawResult>,
}

#[derive(Deserialize, Debug, Default)]
struct RawResult {
    #[serde(default, rename = "Name")]
    name: Option<String>,
    #[serde(default, rename = "Description")]
    description: Option<String>,
    #[serde(default, rename = "NumVotes")]
    num_votes: Option<i64>,
    #[serde(default, rename = "Version")]
    version: Option<String>,
    #[serde(default, rename = "Maintainer")]
    maintainer: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .results
        .into_iter()
        .filter_map(|r| {
            let name = r.name.as_deref().map(str::trim).filter(|n| !n.is_empty())?;
            let url = format!("https://aur.archlinux.org/packages/{name}");
            let mut snippet = String::new();
            if let Some(description) = r
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(description);
                snippet.push_str(". ");
            }
            if let Some(version) = r
                .version
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                snippet.push_str(&format!("v{version}. "));
            }
            if let Some(votes) = r.num_votes {
                snippet.push_str(&format!("{votes} votes. "));
            }
            if let Some(maintainer) = r
                .maintainer
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty())
            {
                snippet.push_str(&format!("Maintainer: {maintainer}. "));
            }
            Some(build_result(ENGINE, &url, name, &snippet))
        })
        .collect())
}

pub struct AurProvider {
    config: Config,
}

impl AurProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for AurProvider {
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
                ("arg", query.to_string()),
                ("type", "search".to_string()),
                ("by", "name-desc".to_string()),
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
            .query(&[("arg", "rust".to_string()), ("type", "search".to_string())])
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
          "Name": "rustup",
          "Description": "The Rust toolchain installer",
          "NumVotes": 500,
          "Version": "1.27.0-1",
          "Maintainer": "foo"
        },
        { "Name": "" }
      ]
    }"#;

    #[test]
    fn parses_packages_and_skips_nameless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "rustup");
        assert!(results[0].snippet.contains("The Rust toolchain installer."));
        assert!(results[0].snippet.contains("v1.27.0-1."));
        assert!(results[0].snippet.contains("500 votes."));
        assert!(results[0].snippet.contains("Maintainer: foo."));
        assert_eq!(results[0].url, "https://aur.archlinux.org/packages/rustup");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
