//! Zenodo adapter (`academic` profile).
//!
//! Public records search API, no key. Returns title, creators, description
//! and a canonical record URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "zenodo";
const ENDPOINT: &str = "https://zenodo.org/api/records";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    hits: RawHits,
}

#[derive(Deserialize, Debug, Default)]
struct RawHits {
    #[serde(default)]
    hits: Vec<RawRecord>,
}

#[derive(Deserialize, Debug, Default)]
struct RawRecord {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    metadata: Option<RawMetadata>,
    #[serde(default)]
    links: Option<RawLinks>,
}

#[derive(Deserialize, Debug, Default)]
struct RawMetadata {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    creators: Option<Vec<RawCreator>>,
    #[serde(default, rename = "publication_date")]
    publication_date: Option<String>,
    #[serde(default, rename = "resource_type")]
    resource_type: Option<RawResourceType>,
}

#[derive(Deserialize, Debug, Default)]
struct RawCreator {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct RawResourceType {
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct RawLinks {
    #[serde(default)]
    html: Option<String>,
    #[serde(default)]
    self_html: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .hits
        .hits
        .into_iter()
        .filter_map(|record| {
            let metadata = record.metadata.unwrap_or_default();
            let title = metadata
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = record
                .links
                .as_ref()
                .and_then(|l| l.html.as_deref().or(l.self_html.as_deref()))
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    record
                        .id
                        .map(|id| format!("https://zenodo.org/records/{id}"))
                })
                .unwrap_or_else(|| "https://zenodo.org".to_string());
            let mut snippet = String::new();
            if let Some(creators) = metadata.creators.as_ref().filter(|c| !c.is_empty()) {
                let names: Vec<&str> = creators
                    .iter()
                    .filter_map(|c| c.name.as_deref().map(str::trim).filter(|n| !n.is_empty()))
                    .collect();
                if !names.is_empty() {
                    snippet.push_str(&names.join(", "));
                    snippet.push_str(". ");
                }
            }
            if let Some(date) = metadata
                .publication_date
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(&format!("{date}. "));
            }
            if let Some(rt) = metadata
                .resource_type
                .as_ref()
                .and_then(|r| r.title.as_deref())
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                snippet.push_str(&format!("Type: {rt}. "));
            }
            if let Some(description) = metadata
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

pub struct ZenodoProvider {
    config: Config,
}

impl ZenodoProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for ZenodoProvider {
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
                ("size", per_page.to_string()),
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
            .query(&[("q", "rust".to_string()), ("size", "1".to_string())])
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
      "hits": {
        "hits": [
          {
            "id": 12345,
            "metadata": {
              "title": "A Rust async runtime paper",
              "description": "We present a new runtime",
              "creators": [{ "name": "Alice Smith" }, { "name": "Bob Jones" }],
              "publication_date": "2024-01-15",
              "resource_type": { "title": "Publication" }
            },
            "links": { "html": "https://zenodo.org/records/12345" }
          },
          { "metadata": { "title": "" } }
        ]
      }
    }"#;

    #[test]
    fn parses_records_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "A Rust async runtime paper");
        assert!(results[0].snippet.contains("Alice Smith, Bob Jones."));
        assert!(results[0].snippet.contains("2024-01-15."));
        assert!(results[0].snippet.contains("Type: Publication."));
        assert!(results[0].snippet.contains("We present a new runtime."));
        assert_eq!(results[0].url, "https://zenodo.org/records/12345");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
