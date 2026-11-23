//! Wikisource adapter (`knowledge` profile).
//!
//! Public MediaWiki search API, no key. Returns source texts with title and
//! a canonical wikisource.org URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "wikisource";
const ENDPOINT: &str = "https://en.wikisource.org/w/api.php";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    query: Option<RawQuery>,
}

#[derive(Deserialize, Debug, Default)]
struct RawQuery {
    #[serde(default)]
    search: Vec<RawSearch>,
}

#[derive(Deserialize, Debug, Default)]
struct RawSearch {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    snippet: Option<String>,
    #[serde(default)]
    pageid: Option<i64>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    let search = parsed.query.map(|q| q.search).unwrap_or_default();
    Ok(search
        .into_iter()
        .filter_map(|s| {
            let title = s.title.as_deref().map(str::trim).filter(|t| !t.is_empty())?;
            let url = s
                .pageid
                .map(|id| format!("https://en.wikisource.org/?curid={id}"))
                .unwrap_or_else(|| "https://en.wikisource.org".to_string());
            let snippet = s.snippet.as_deref().map(str::trim).filter(|sn| !sn.is_empty()).unwrap_or("");
            Some(build_result(ENGINE, &url, title, snippet))
        })
        .collect())
}

pub struct WikisourceProvider {
    config: Config,
}

impl WikisourceProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for WikisourceProvider {
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
                ("action", "query".to_string()),
                ("list", "search".to_string()),
                ("srsearch", query.to_string()),
                ("srlimit", per_page.to_string()),
                ("sroffset", ((page.saturating_sub(1)) * per_page).to_string()),
                ("format", "json".to_string()),
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
                ("action", "query".to_string()),
                ("list", "search".to_string()),
                ("srsearch", "rust".to_string()),
                ("srlimit", "1".to_string()),
                ("format", "json".to_string()),
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
      "query": {
        "search": [
          { "title": "Rust (poem)", "snippet": "A poem about rust", "pageid": 123 },
          { "title": "" }
        ]
      }
    }"#;

    #[test]
    fn parses_results_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust (poem)");
        assert!(results[0].snippet.contains("A poem about rust"));
        assert_eq!(results[0].url, "https://en.wikisource.org/?curid=123");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
