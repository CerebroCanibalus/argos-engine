//! Internet Archive adapter (`knowledge`/`general` profiles).
//!
//! Public search API, no key. Returns items with title, creator, date and
//! a canonical archive.org details URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "internetarchive";
const ENDPOINT: &str = "https://archive.org/advancedsearch.php";

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
    #[serde(default, rename = "identifier")]
    identifier: Option<String>,
    #[serde(default, rename = "title")]
    title: Option<String>,
    #[serde(default, rename = "creator")]
    creator: Option<flojo_mcp::serde_json::Value>,
    #[serde(default, rename = "date")]
    date: Option<String>,
    #[serde(default, rename = "description")]
    description: Option<String>,
    #[serde(default, rename = "mediatype")]
    mediatype: Option<String>,
}

fn creators_of(v: &Option<flojo_mcp::serde_json::Value>) -> Vec<String> {
    match v {
        Some(flojo_mcp::serde_json::Value::String(s)) => vec![s.clone()],
        Some(flojo_mcp::serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
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
            let id = doc
                .identifier
                .as_deref()
                .map(str::trim)
                .filter(|i| !i.is_empty())?;
            let url = format!("https://archive.org/details/{id}");
            let mut snippet = String::new();
            let creators = creators_of(&doc.creator);
            if !creators.is_empty() {
                snippet.push_str(&creators.join(", "));
                snippet.push_str(". ");
            }
            if let Some(date) = doc.date.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
                snippet.push_str(&format!("{date}. "));
            }
            if let Some(mediatype) = doc
                .mediatype
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty())
            {
                snippet.push_str(&format!("Type: {mediatype}. "));
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

pub struct InternetArchiveProvider {
    config: Config,
}

impl InternetArchiveProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for InternetArchiveProvider {
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
                ("q", query.to_string()),
                ("fl[]", "identifier".to_string()),
                ("fl[]", "title".to_string()),
                ("fl[]", "creator".to_string()),
                ("fl[]", "date".to_string()),
                ("fl[]", "description".to_string()),
                ("fl[]", "mediatype".to_string()),
                ("rows", per_page.to_string()),
                ("page", page.to_string()),
                ("output", "json".to_string()),
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
                ("output", "json".to_string()),
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
            "identifier": "rust-book",
            "title": "The Rust Book",
            "creator": ["Steve Klabnik", "Carol Nichols"],
            "date": "2018",
            "mediatype": "texts",
            "description": "A guide to Rust"
          },
          { "identifier": "x", "title": "" }
        ]
      }
    }"#;

    #[test]
    fn parses_items_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "The Rust Book");
        assert!(results[0].snippet.contains("Steve Klabnik, Carol Nichols."));
        assert!(results[0].snippet.contains("2018."));
        assert!(results[0].snippet.contains("Type: texts."));
        assert!(results[0].snippet.contains("A guide to Rust."));
        assert_eq!(results[0].url, "https://archive.org/details/rust-book");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
