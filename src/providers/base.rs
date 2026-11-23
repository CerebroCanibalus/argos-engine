//! BASE (Bielefeld Academic Search Engine) adapter (`academic` profile).
//!
//! Public JSON search interface, no key. Returns title, author, date and a
//! canonical record URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "base";
const ENDPOINT: &str = "https://api.base-search.net/cgi-bin/BaseHttpSearchInterface.fcgi";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    response: RawResponseInner,
}

#[derive(Deserialize, Debug, Default)]
struct RawResponseInner {
    #[serde(default)]
    docs: Vec<RawDoc>,
}

#[derive(Deserialize, Debug, Default)]
struct RawDoc {
    #[serde(default, rename = "dc:title")]
    title: Option<String>,
    #[serde(default, rename = "dc:creator")]
    creator: Option<Vec<String>>,
    #[serde(default, rename = "dc:date")]
    date: Option<String>,
    #[serde(default, rename = "dc:identifier")]
    identifier: Option<Vec<String>>,
    #[serde(default, rename = "dc:description")]
    description: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .response
        .docs
        .into_iter()
        .filter_map(|doc| {
            let title = doc
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = doc
                .identifier
                .as_ref()
                .and_then(|ids| {
                    ids.iter()
                        .map(|i| i.trim())
                        .find(|i| i.starts_with("http"))
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "https://www.base-search.net".to_string());
            let mut snippet = String::new();
            if let Some(creators) = doc.creator.as_ref().filter(|c| !c.is_empty()) {
                snippet.push_str(&creators.join(", "));
                snippet.push_str(". ");
            }
            if let Some(date) = doc.date.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
                snippet.push_str(&format!("{date}. "));
            }
            if let Some(description) = doc
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(description);
                snippet.push_str(". ");
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct BaseProvider {
    config: Config,
}

impl BaseProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for BaseProvider {
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
                ("func", "PerformSearch".to_string()),
                ("query", query.to_string()),
                ("format", "json".to_string()),
                ("hits", per_page.to_string()),
                ("offset", ((page.saturating_sub(1)) * per_page).to_string()),
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
                ("func", "PerformSearch".to_string()),
                ("query", "rust".to_string()),
                ("format", "json".to_string()),
                ("hits", "1".to_string()),
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
            "dc:title": "Rust concurrency patterns",
            "dc:creator": ["Carol Nichols"],
            "dc:date": "2023",
            "dc:identifier": ["https://example.com/paper", "doi:10.1/x"],
            "dc:description": "A survey of concurrency in Rust"
          },
          { "dc:title": "" }
        ]
      }
    }"#;

    #[test]
    fn parses_documents_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust concurrency patterns");
        assert!(results[0].snippet.contains("Carol Nichols."));
        assert!(results[0].snippet.contains("2023."));
        assert!(
            results[0]
                .snippet
                .contains("A survey of concurrency in Rust.")
        );
        assert_eq!(results[0].url, "https://example.com/paper");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
