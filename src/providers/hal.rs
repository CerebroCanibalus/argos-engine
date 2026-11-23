//! HAL (French open archive) adapter (`academic` profile).
//!
//! Public search API, no key. Returns documents with title, authors,
//! date and a canonical hal.science URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "hal";
const ENDPOINT: &str = "https://api.archives-ouvertes.fr/search";

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
    #[serde(default, rename = "title_s")]
    title: Option<Vec<String>>,
    #[serde(default, rename = "authFullName_s")]
    authors: Option<Vec<String>>,
    #[serde(default, rename = "producedDateY_i")]
    year: Option<i64>,
    #[serde(default, rename = "halId_s")]
    hal_id: Option<String>,
    #[serde(default, rename = "docType_s")]
    doc_type: Option<String>,
    #[serde(default, rename = "abstract_s")]
    abstract_: Option<Vec<String>>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .response
        .docs
        .into_iter()
        .filter_map(|d| {
            let title = d
                .title
                .as_ref()
                .and_then(|t| t.first())
                .map(|s| s.trim())
                .filter(|t| !t.is_empty())?;
            let hal_id = d
                .hal_id
                .as_deref()
                .map(str::trim)
                .filter(|h| !h.is_empty())
                .unwrap_or("");
            let url = if hal_id.is_empty() {
                "https://hal.science".to_string()
            } else {
                format!("https://hal.science/{hal_id}")
            };
            let mut snippet = String::new();
            if let Some(authors) = d.authors.as_ref().filter(|a| !a.is_empty()) {
                snippet.push_str(&authors.join(", "));
                snippet.push_str(". ");
            }
            if let Some(year) = d.year {
                snippet.push_str(&format!("{year}. "));
            }
            if let Some(dt) = d
                .doc_type
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                snippet.push_str(&format!("Type: {dt}. "));
            }
            if let Some(abs) = d
                .abstract_
                .as_ref()
                .and_then(|a| a.first())
                .map(|s| s.trim())
                .filter(|a| !a.is_empty())
            {
                snippet.push_str(abs);
                snippet.push_str(". ");
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct HalProvider {
    config: Config,
}

impl HalProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for HalProvider {
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
                ("rows", per_page.to_string()),
                ("start", ((page.saturating_sub(1)) * per_page).to_string()),
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
            "title_s": ["A Rust concurrency paper"],
            "authFullName_s": ["Alice Smith", "Bob Jones"],
            "producedDateY_i": 2024,
            "halId_s": "hal-01234567",
            "docType_s": "ART",
            "abstract_s": ["We study concurrency"]
          },
          { "title_s": [""] }
        ]
      }
    }"#;

    #[test]
    fn parses_docs_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "A Rust concurrency paper");
        assert!(results[0].snippet.contains("Alice Smith, Bob Jones."));
        assert!(results[0].snippet.contains("2024."));
        assert!(results[0].snippet.contains("Type: ART."));
        assert!(results[0].snippet.contains("We study concurrency."));
        assert_eq!(results[0].url, "https://hal.science/hal-01234567");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
