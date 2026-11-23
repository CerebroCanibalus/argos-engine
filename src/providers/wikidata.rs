//! Wikidata adapter (`knowledge` profile).
//!
//! The public `wbsearchentities` endpoint needs no key and returns the
//! canonical entity, its description and a sitelink. It is the structured
//! backbone behind Wikipedia's free-form articles.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "wikidata";
const ENDPOINT: &str = "https://www.wikidata.org/w/api.php";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    search: Vec<RawEntity>,
}

#[derive(Deserialize, Debug, Default)]
struct RawEntity {
    #[serde(default)]
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "concepturi")]
    concept_uri: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .search
        .into_iter()
        .filter_map(|entity| {
            let title = entity.label.trim();
            if title.is_empty() {
                return None;
            }
            let id = entity.id.trim();
            let url = entity
                .concept_uri
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("https://www.wikidata.org/wiki/{id}"));
            let mut snippet = String::new();
            if let Some(description) = entity
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(description);
                snippet.push_str(". ");
            }
            snippet.push_str(&format!("Wikidata ID: {id}."));
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct WikidataProvider {
    config: Config,
}

impl WikidataProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for WikidataProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 50);
        let offset = (page.saturating_sub(1)) * per_page;
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("action", "wbsearchentities".to_string()),
                ("search", query.to_string()),
                ("language", "en".to_string()),
                ("format", "json".to_string()),
                ("limit", per_page.to_string()),
                ("offset", offset.to_string()),
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
                ("action", "wbsearchentities".to_string()),
                ("search", "rust".to_string()),
                ("language", "en".to_string()),
                ("format", "json".to_string()),
                ("limit", "1".to_string()),
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
      "search": [
        {
          "id": "Q162243",
          "label": "Rust (programming language)",
          "description": "multi-paradigm programming language",
          "concepturi": "http://www.wikidata.org/entity/Q162243"
        },
        { "id": "Q47529", "label": "", "description": "no label" }
      ]
    }"#;

    #[test]
    fn parses_entities_and_skips_labelless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust (programming language)");
        assert!(
            results[0]
                .snippet
                .contains("multi-paradigm programming language.")
        );
        assert!(results[0].snippet.contains("Wikidata ID: Q162243."));
        assert_eq!(results[0].url, "http://www.wikidata.org/entity/Q162243");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
