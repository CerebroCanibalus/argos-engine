//! ORCID adapter (`academic` profile).
//!
//! Public person search endpoint, no key. Returns researchers with their
//! ORCID iD and a canonical profile URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "orcid";
const ENDPOINT: &str = "https://pub.orcid.org/v3.0/search";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    result: Vec<RawResult>,
}

#[derive(Deserialize, Debug, Default)]
struct RawResult {
    #[serde(default, rename = "orcid-identifier")]
    orcid_identifier: Option<RawOrcidId>,
}

#[derive(Deserialize, Debug, Default)]
struct RawOrcidId {
    #[serde(default, rename = "path")]
    path: Option<String>,
    #[serde(default, rename = "uri")]
    uri: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .result
        .into_iter()
        .filter_map(|r| {
            let id = r
                .orcid_identifier
                .as_ref()
                .and_then(|o| o.path.as_deref().or(o.uri.as_deref()))
                .map(str::trim)
                .filter(|s| !s.is_empty())?;
            let url = format!("https://orcid.org/{id}");
            let snippet = format!("ORCID iD: {id}.");
            Some(build_result(ENGINE, &url, id, &snippet))
        })
        .collect())
}

pub struct OrcidProvider {
    config: Config,
}

impl OrcidProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for OrcidProvider {
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
            ])
            .header("Accept", "application/json")
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
            .query(&[("q", "rust".to_string()), ("rows", "1".to_string())])
            .header("Accept", "application/json")
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
      "result": [
        { "orcid-identifier": { "path": "0000-0002-1825-0097", "uri": "https://orcid.org/0000-0002-1825-0097" } },
        { "orcid-identifier": { "path": "" } }
      ]
    }"#;

    #[test]
    fn parses_researchers_and_skips_idless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "0000-0002-1825-0097");
        assert!(
            results[0]
                .snippet
                .contains("ORCID iD: 0000-0002-1825-0097.")
        );
        assert_eq!(results[0].url, "https://orcid.org/0000-0002-1825-0097");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
