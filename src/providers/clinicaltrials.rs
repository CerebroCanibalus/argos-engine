//! ClinicalTrials.gov adapter (`academic` profile).
//!
//! Public search API v2, no key. Returns studies with title, status and a
//! canonical clinicaltrials.gov URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "clinicaltrials";
const ENDPOINT: &str = "https://clinicaltrials.gov/api/v2/studies";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    studies: Vec<RawStudy>,
}

#[derive(Deserialize, Debug, Default)]
#[allow(non_snake_case)]
struct RawStudy {
    #[serde(default)]
    #[serde(rename = "protocolSection")]
    protocolSection: Option<RawProtocol>,
}

#[derive(Deserialize, Debug, Default)]
#[allow(non_snake_case)]
struct RawProtocol {
    #[serde(default)]
    #[serde(rename = "identificationModule")]
    identificationModule: Option<RawIdentification>,
    #[serde(default)]
    #[serde(rename = "statusModule")]
    statusModule: Option<RawStatus>,
}

#[derive(Deserialize, Debug, Default)]
#[allow(non_snake_case)]
struct RawIdentification {
    #[serde(default)]
    #[serde(rename = "nctId")]
    nctId: Option<String>,
    #[serde(default)]
    #[serde(rename = "briefTitle")]
    briefTitle: Option<String>,
    #[serde(default)]
    #[serde(rename = "officialTitle")]
    officialTitle: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
#[allow(non_snake_case)]
struct RawStatus {
    #[serde(default)]
    #[serde(rename = "overallStatus")]
    overallStatus: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .studies
        .into_iter()
        .filter_map(|s| {
            let proto = s.protocolSection.unwrap_or_default();
            let ident = proto.identificationModule.unwrap_or_default();
            let title = ident
                .briefTitle
                .as_deref()
                .or(ident.officialTitle.as_deref())
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let nct = ident
                .nctId
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .unwrap_or("");
            let url = if nct.is_empty() {
                "https://clinicaltrials.gov".to_string()
            } else {
                format!("https://clinicaltrials.gov/study/{nct}")
            };
            let mut snippet = String::new();
            if !nct.is_empty() {
                snippet.push_str(&format!("{nct}. "));
            }
            if let Some(status) = proto
                .statusModule
                .as_ref()
                .and_then(|s| s.overallStatus.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                snippet.push_str(&format!("Status: {status}. "));
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct ClinicalTrialsProvider {
    config: Config,
}

impl ClinicalTrialsProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for ClinicalTrialsProvider {
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
                ("query.term", query.to_string()),
                ("pageSize", per_page.to_string()),
                (
                    "pageToken",
                    if page <= 1 {
                        String::new()
                    } else {
                        format!("page={page}")
                    },
                ),
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
                ("query.term", "cancer".to_string()),
                ("pageSize", "1".to_string()),
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
      "studies": [
        {
          "protocolSection": {
            "identificationModule": {
              "nctId": "NCT01234567",
              "briefTitle": "A trial of drug X"
            },
            "statusModule": { "overallStatus": "Recruiting" }
          }
        },
        { "protocolSection": { "identificationModule": { "briefTitle": "" } } }
      ]
    }"#;

    #[test]
    fn parses_studies_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "A trial of drug X");
        assert!(results[0].snippet.contains("NCT01234567."));
        assert!(results[0].snippet.contains("Status: Recruiting."));
        assert_eq!(
            results[0].url,
            "https://clinicaltrials.gov/study/NCT01234567"
        );
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
