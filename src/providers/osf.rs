//! OSF (Open Science Framework) adapter (`academic` profile).
//!
//! Public search API, no key. Returns registrations/preprints with title,
//! description and a canonical osf.io URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "osf";
const ENDPOINT: &str = "https://api.osf.io/v2/search/";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    data: Vec<RawItem>,
}

#[derive(Deserialize, Debug, Default)]
struct RawItem {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    attributes: Option<RawAttrs>,
    #[serde(default)]
    links: Option<RawLinks>,
}

#[derive(Deserialize, Debug, Default)]
struct RawAttrs {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    date_created: Option<String>,
    #[serde(default, rename = "resource_type")]
    resource_type: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct RawLinks {
    #[serde(default)]
    html: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .data
        .into_iter()
        .filter_map(|item| {
            let attrs = item.attributes.unwrap_or_default();
            let title = attrs
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = item
                .links
                .as_ref()
                .and_then(|l| l.html.as_deref())
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .or_else(|| item.id.as_deref().map(|id| format!("https://osf.io/{id}")))
                .unwrap_or_else(|| "https://osf.io".to_string());
            let mut snippet = String::new();
            if let Some(rt) = attrs
                .resource_type
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
            {
                snippet.push_str(&format!("Type: {rt}. "));
            }
            if let Some(date) = attrs
                .date_created
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(&format!("{date}. "));
            }
            if let Some(desc) = attrs
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(desc);
                snippet.push_str(". ");
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct OsfProvider {
    config: Config,
}

impl OsfProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for OsfProvider {
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
                ("page[size]", per_page.to_string()),
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
            .query(&[("q", "rust".to_string()), ("page[size]", "1".to_string())])
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
          "id": "abc12",
          "attributes": {
            "title": "Open Rust data",
            "description": "A shared dataset",
            "date_created": "2024-01-01",
            "resource_type": "Project"
          },
          "links": { "html": "https://osf.io/abc12" }
        },
        { "attributes": { "title": "" } }
      ]
    }"#;

    #[test]
    fn parses_items_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Open Rust data");
        assert!(results[0].snippet.contains("Type: Project."));
        assert!(results[0].snippet.contains("2024-01-01."));
        assert!(results[0].snippet.contains("A shared dataset."));
        assert_eq!(results[0].url, "https://osf.io/abc12");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
