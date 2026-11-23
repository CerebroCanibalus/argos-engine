//! World Bank adapter (`knowledge` profile).
//!
//! Public indicators/projects search, no key. Returns project name,
//! country, status and a canonical projects.worldbank.org URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "worldbank";
const ENDPOINT: &str = "https://search.worldbank.org/api/v2/projects";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    projects: Option<std::collections::HashMap<String, RawProject>>,
}

#[derive(Deserialize, Debug, Default)]
struct RawProject {
    #[serde(default, rename = "project_name")]
    project_name: Option<String>,
    #[serde(default, rename = "countryname")]
    country_name: Option<String>,
    #[serde(default, rename = "status")]
    status: Option<String>,
    #[serde(default, rename = "project_abstract")]
    abstract_: Option<String>,
    #[serde(default, rename = "boardapprovaldate")]
    approval_date: Option<String>,
    #[serde(default, rename = "url")]
    url: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    let projects = parsed.projects.unwrap_or_default();
    Ok(projects
        .into_values()
        .filter_map(|p| {
            let title = p
                .project_name
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = p
                .url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| "https://projects.worldbank.org".to_string());
            let mut snippet = String::new();
            if let Some(country) = p
                .country_name
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
            {
                snippet.push_str(&format!("{country}. "));
            }
            if let Some(status) = p.status.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                snippet.push_str(&format!("Status: {status}. "));
            }
            if let Some(date) = p
                .approval_date
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(&format!("Approved: {date}. "));
            }
            if let Some(abs) = p
                .abstract_
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())
            {
                snippet.push_str(abs);
                snippet.push_str(". ");
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct WorldBankProvider {
    config: Config,
}

impl WorldBankProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for WorldBankProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 50);
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("format", "json".to_string()),
                ("qterm", query.to_string()),
                ("rows", per_page.to_string()),
                ("os", ((page.saturating_sub(1)) * per_page).to_string()),
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
        // World Bank returns [metadata, {projects: {...}}]
        let projects_payload = if let Ok(arr) =
            flojo_mcp::serde_json::from_str::<Vec<flojo_mcp::serde_json::Value>>(&payload)
        {
            arr.get(1)
                .cloned()
                .unwrap_or(flojo_mcp::serde_json::Value::Object(Default::default()))
        } else {
            flojo_mcp::serde_json::from_str(&payload)
                .unwrap_or(flojo_mcp::serde_json::Value::Object(Default::default()))
        };
        let text = projects_payload.to_string();
        let mut results = parse_results(&text)?;
        results.truncate(limit);
        Ok(results)
    }

    async fn health(&self) -> bool {
        match api_client()
            .get(ENDPOINT)
            .query(&[
                ("format", "json".to_string()),
                ("qterm", "water".to_string()),
                ("rows", "1".to_string()),
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

    const FIXTURE: &str = r#"{ "projects": {
      "p1": {
        "project_name": "Water Supply Project",
        "countryname": "Kenya",
        "status": "Active",
        "boardapprovaldate": "2023-01-01",
        "project_abstract": "Improve water access",
        "url": "https://projects.worldbank.org/en/projects/P123"
      },
      "p2": { "project_name": "" }
    } }"#;

    #[test]
    fn parses_projects_and_skips_nameless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Water Supply Project");
        assert!(results[0].snippet.contains("Kenya."));
        assert!(results[0].snippet.contains("Status: Active."));
        assert!(results[0].snippet.contains("Approved: 2023-01-01."));
        assert!(results[0].snippet.contains("Improve water access."));
        assert_eq!(
            results[0].url,
            "https://projects.worldbank.org/en/projects/P123"
        );
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
