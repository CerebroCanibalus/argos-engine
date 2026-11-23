//! OpenAIRE adapter (`academic` profile).
//!
//! Public publications search endpoint, no key. Returns title, creators,
//! date and a canonical record URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "openaire";
const ENDPOINT: &str = "https://api.openaire.eu/search/publications";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default, rename = "response")]
    response: RawOuter,
}

#[derive(Deserialize, Debug, Default)]
struct RawOuter {
    #[serde(default)]
    results: RawResults,
}

#[derive(Deserialize, Debug, Default)]
struct RawResults {
    #[serde(default, rename = "result")]
    result: Vec<RawResult>,
}

#[derive(Deserialize, Debug, Default)]
struct RawResult {
    #[serde(default, rename = "header")]
    header: Option<RawHeader>,
    #[serde(default, rename = "metadata")]
    metadata: Option<RawMetadata>,
}

#[derive(Deserialize, Debug, Default)]
struct RawHeader {
    #[serde(default, rename = "dri:objIdentifier")]
    obj_identifier: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct RawMetadata {
    #[serde(default, rename = "oaf:entity")]
    entity: Option<RawEntity>,
}

#[derive(Deserialize, Debug, Default)]
struct RawEntity {
    #[serde(default, rename = "oaf:result")]
    result: Option<RawResultBody>,
}

#[derive(Deserialize, Debug, Default)]
struct RawResultBody {
    #[serde(default, rename = "title")]
    title: Option<RawValue>,
    #[serde(default, rename = "creator")]
    creator: Option<RawCreatorField>,
    #[serde(default, rename = "dateofacceptance")]
    date_of_acceptance: Option<RawValue>,
    #[serde(default, rename = "description")]
    description: Option<RawValue>,
}

#[derive(Deserialize, Debug, Default)]
struct RawValue {
    #[serde(default, rename = "$")]
    value: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(untagged)]
enum RawCreatorField {
    Text(String),
    Obj {
        value: Option<String>,
    },
    List(Vec<RawCreatorObj>),
    #[default]
    None,
}

#[derive(Deserialize, Debug, Default)]
struct RawCreatorObj {
    #[serde(default, rename = "$")]
    value: Option<String>,
}

impl RawCreatorField {
    fn names(&self) -> Vec<String> {
        match self {
            RawCreatorField::Text(s) => vec![s.clone()],
            RawCreatorField::Obj { value } => value.clone().into_iter().collect(),
            RawCreatorField::List(items) => items.iter().filter_map(|c| c.value.clone()).collect(),
            RawCreatorField::None => Vec::new(),
        }
    }
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .response
        .results
        .result
        .into_iter()
        .filter_map(|r| {
            let body = r.metadata?.entity?.result?;
            let title = body
                .title
                .as_ref()
                .and_then(|v| v.value.as_deref())
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = r
                .header
                .as_ref()
                .and_then(|h| h.obj_identifier.as_deref())
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| "https://www.openaire.eu".to_string());
            let mut snippet = String::new();
            let creators = body
                .creator
                .as_ref()
                .map(RawCreatorField::names)
                .unwrap_or_default();
            if !creators.is_empty() {
                snippet.push_str(&creators.join(", "));
                snippet.push_str(". ");
            }
            if let Some(date) = body
                .date_of_acceptance
                .as_ref()
                .and_then(|v| v.value.as_deref())
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(&format!("{date}. "));
            }
            if let Some(description) = body
                .description
                .as_ref()
                .and_then(|v| v.value.as_deref())
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

pub struct OpenaireProvider {
    config: Config,
}

impl OpenaireProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for OpenaireProvider {
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
                ("keywords", query.to_string()),
                ("format", "json".to_string()),
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
            .query(&[
                ("keywords", "rust".to_string()),
                ("format", "json".to_string()),
                ("size", "1".to_string()),
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
        "results": {
          "result": [
            {
              "header": { "dri:objIdentifier": "https://doi.org/10.1234/x" },
              "metadata": {
                "oaf:entity": {
                  "oaf:result": {
                    "title": { "$": "Rust runtime paper" },
                    "creator": { "value": "Alice Smith" },
                    "dateofacceptance": { "$": "2024-02-01" },
                    "description": { "$": "We study runtimes" }
                  }
                }
              }
            },
            { "metadata": {} }
          ]
        }
      }
    }"#;

    #[test]
    fn parses_publications_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust runtime paper");
        assert!(results[0].snippet.contains("Alice Smith."));
        assert!(results[0].snippet.contains("2024-02-01."));
        assert!(results[0].snippet.contains("We study runtimes."));
        assert_eq!(results[0].url, "https://doi.org/10.1234/x");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
