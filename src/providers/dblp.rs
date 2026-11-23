//! dblp adapter (`academic` profile).
//!
//! dblp is the computer-science bibliography. Public JSON API, no key, returns
//! title, authors, venue and year for matching publications.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "dblp";
const ENDPOINT: &str = "https://dblp.org/search/publ/api";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    result: RawResult,
}

#[derive(Deserialize, Debug, Default)]
struct RawResult {
    #[serde(default)]
    hits: RawHits,
}

#[derive(Deserialize, Debug, Default)]
struct RawHits {
    #[serde(default)]
    hit: Vec<RawHit>,
}

#[derive(Deserialize, Debug, Default)]
struct RawHit {
    #[serde(default)]
    info: RawInfo,
}

#[derive(Deserialize, Debug, Default)]
struct RawInfo {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    authors: Option<RawAuthors>,
    #[serde(default)]
    venue: Option<String>,
    #[serde(default)]
    year: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    doi: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct RawAuthors {
    #[serde(default)]
    author: Vec<StringOrObj>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(untagged)]
enum StringOrObj {
    Text(String),
    Obj {
        text: Option<String>,
    },
    #[default]
    None,
}

impl StringOrObj {
    fn as_str(&self) -> Option<&str> {
        match self {
            StringOrObj::Text(s) => Some(s.as_str()),
            StringOrObj::Obj { text } => text.as_deref(),
            StringOrObj::None => None,
        }
    }
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .result
        .hits
        .hit
        .into_iter()
        .filter_map(|hit| {
            let info = hit.info;
            let title = info
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = info
                .url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    info.doi
                        .as_deref()
                        .map(str::trim)
                        .filter(|d| !d.is_empty())
                        .map(|d| format!("https://doi.org/{d}"))
                })
                .unwrap_or_else(|| "https://dblp.org".to_string());
            let mut snippet = String::new();
            if let Some(authors) = info.authors.as_ref() {
                let names: Vec<&str> = authors
                    .author
                    .iter()
                    .filter_map(StringOrObj::as_str)
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .collect();
                if !names.is_empty() {
                    snippet.push_str(&names.join(", "));
                    snippet.push_str(". ");
                }
            }
            if let Some(venue) = info
                .venue
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                snippet.push_str(venue);
                snippet.push_str(". ");
            }
            if let Some(year) = info
                .year
                .as_deref()
                .map(str::trim)
                .filter(|y| !y.is_empty())
            {
                snippet.push_str(&format!("({year}). "));
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct DblpProvider {
    config: Config,
}

impl DblpProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for DblpProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 100);
        let offset = (page.saturating_sub(1)) * per_page;
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("q", query.to_string()),
                ("format", "json".to_string()),
                ("h", per_page.to_string()),
                ("f", offset.to_string()),
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
                ("format", "json".to_string()),
                ("h", "1".to_string()),
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
      "result": {
        "hits": {
          "hit": [
            {
              "info": {
                "title": "Rust: A Systems Programming Language",
                "authors": { "author": ["Graydon Hoare", "Patrick Walton"] },
                "venue": "OOPSLA",
                "year": "2014",
                "url": "https://dblp.org/rec/conf/oopsla/HoareW14",
                "doi": "10.1145/2663171.2663188"
              }
            },
            { "info": { "title": "" } }
          ]
        }
      }
    }"#;

    #[test]
    fn parses_publications_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust: A Systems Programming Language");
        assert!(
            results[0]
                .snippet
                .contains("Graydon Hoare, Patrick Walton.")
        );
        assert!(results[0].snippet.contains("OOPSLA."));
        assert!(results[0].snippet.contains("(2014)."));
        assert_eq!(results[0].url, "https://dblp.org/rec/conf/oopsla/HoareW14");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
