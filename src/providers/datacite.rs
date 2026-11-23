//! DataCite adapter (`academic` profile).
//!
//! Public DOI metadata search, no key. Returns works with title, creators,
//! publisher and a canonical doi.org URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "datacite";
const ENDPOINT: &str = "https://api.datacite.org/dois";

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
}

#[derive(Deserialize, Debug, Default)]
struct RawAttrs {
    #[serde(default)]
    doi: Option<String>,
    #[serde(default)]
    titles: Option<Vec<RawTitle>>,
    #[serde(default)]
    creators: Option<Vec<RawCreator>>,
    #[serde(default)]
    publisher: Option<String>,
    #[serde(default)]
    publication_year: Option<i64>,
    #[serde(default)]
    descriptions: Option<Vec<RawDescription>>,
}

#[derive(Deserialize, Debug, Default)]
struct RawTitle {
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct RawCreator {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct RawDescription {
    #[serde(default)]
    description: Option<String>,
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
                .titles
                .as_ref()
                .and_then(|t| t.first())
                .and_then(|t| t.title.as_deref())
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let doi = attrs
                .doi
                .as_deref()
                .or(item.id.as_deref())
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .unwrap_or("");
            let url = if doi.is_empty() {
                "https://datacite.org".to_string()
            } else {
                format!("https://doi.org/{doi}")
            };
            let mut snippet = String::new();
            if let Some(creators) = attrs.creators.as_ref().filter(|c| !c.is_empty()) {
                let names: Vec<&str> = creators
                    .iter()
                    .filter_map(|c| c.name.as_deref().map(str::trim).filter(|n| !n.is_empty()))
                    .collect();
                if !names.is_empty() {
                    snippet.push_str(&names.join(", "));
                    snippet.push_str(". ");
                }
            }
            if let Some(publisher) = attrs
                .publisher
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
            {
                snippet.push_str(&format!("{publisher}. "));
            }
            if let Some(year) = attrs.publication_year {
                snippet.push_str(&format!("{year}. "));
            }
            if let Some(desc) = attrs
                .descriptions
                .as_ref()
                .and_then(|d| d.first())
                .and_then(|d| d.description.as_deref())
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

pub struct DataCiteProvider {
    config: Config,
}

impl DataCiteProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for DataCiteProvider {
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
                ("query", query.to_string()),
                ("page[size]", per_page.to_string()),
                ("page[number]", page.to_string()),
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
                ("query", "rust".to_string()),
                ("page[size]", "1".to_string()),
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
      "data": [
        {
          "id": "10.5281/zenodo.1",
          "attributes": {
            "doi": "10.5281/zenodo.1",
            "titles": [{ "title": "A Rust dataset" }],
            "creators": [{ "name": "Alice Smith" }],
            "publisher": "Zenodo",
            "publication_year": 2024,
            "descriptions": [{ "description": "Benchmark data" }]
          }
        },
        { "attributes": { "titles": [{ "title": "" }] } }
      ]
    }"#;

    #[test]
    fn parses_works_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "A Rust dataset");
        assert!(results[0].snippet.contains("Alice Smith."));
        assert!(results[0].snippet.contains("Zenodo."));
        assert!(results[0].snippet.contains("2024."));
        assert!(results[0].snippet.contains("Benchmark data."));
        assert_eq!(results[0].url, "https://doi.org/10.5281/zenodo.1");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
