//! NuGet adapter (`code` profile).
//!
//! Public v3 search endpoint, no key. Returns package id, version and
//! description with a canonical nuget.org URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "nuget";
const ENDPOINT: &str = "https://api.nuget.org/v3/search/query";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    data: Vec<RawPackage>,
}

#[derive(Deserialize, Debug, Default)]
struct RawPackage {
    #[serde(default, rename = "id")]
    id: String,
    #[serde(default, rename = "version")]
    version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    authors: Option<Vec<String>>,
    #[serde(default, rename = "totalDownloads")]
    total_downloads: Option<u64>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .data
        .into_iter()
        .filter_map(|pkg| {
            let id = pkg.id.trim();
            if id.is_empty() {
                return None;
            }
            let url = format!("https://www.nuget.org/packages/{id}");
            let mut snippet = String::new();
            if !pkg.version.trim().is_empty() {
                snippet.push_str(&format!("v{}. ", pkg.version.trim()));
            }
            if let Some(description) = pkg
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(description);
                snippet.push_str(". ");
            }
            if let Some(authors) = pkg.authors.as_ref().filter(|a| !a.is_empty()) {
                snippet.push_str(&format!("By {}. ", authors.join(", ")));
            }
            if let Some(downloads) = pkg.total_downloads {
                snippet.push_str(&format!("{downloads} downloads. "));
            }
            Some(build_result(ENGINE, &url, id, &snippet))
        })
        .collect())
}

pub struct NugetProvider {
    config: Config,
}

impl NugetProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for NugetProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 100);
        let skip = (page.saturating_sub(1)) * per_page;
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("q", query.to_string()),
                ("take", per_page.to_string()),
                ("skip", skip.to_string()),
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
            .query(&[("q", "json".to_string()), ("take", "1".to_string())])
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
      "data": [
        {
          "id": "Newtonsoft.Json",
          "version": "13.0.3",
          "description": "Json.NET is a popular high-performance JSON framework",
          "authors": ["James Newton-King"],
          "totalDownloads": 5000000
        },
        { "id": "", "version": "1.0" }
      ]
    }"#;

    #[test]
    fn parses_packages_and_skips_idless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Newtonsoft.Json");
        assert!(results[0].snippet.contains("v13.0.3."));
        assert!(results[0].snippet.contains("Json.NET is a popular"));
        assert!(results[0].snippet.contains("By James Newton-King."));
        assert!(results[0].snippet.contains("5000000 downloads."));
        assert_eq!(
            results[0].url,
            "https://www.nuget.org/packages/Newtonsoft.Json"
        );
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
