//! Figshare adapter (`academic` profile).
//!
//! Public search API, no key. Returns articles with title, authors, description
//! and a canonical figshare.com URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "figshare";
const ENDPOINT: &str = "https://api.figshare.com/v2/articles/search";

#[derive(Deserialize, Debug, Default)]
struct RawArticle {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    published_date: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    defined_type_name: Option<String>,
    #[serde(default)]
    authors: Option<Vec<RawAuthor>>,
}

#[derive(Deserialize, Debug, Default)]
struct RawAuthor {
    #[serde(default)]
    full_name: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: Vec<RawArticle> = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .into_iter()
        .filter_map(|a| {
            let title = a
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = a
                .url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .or_else(|| a.id.map(|id| format!("https://figshare.com/articles/{id}")))
                .unwrap_or_else(|| "https://figshare.com".to_string());
            let mut snippet = String::new();
            if let Some(authors) = a.authors.as_ref().filter(|au| !au.is_empty()) {
                let names: Vec<&str> = authors
                    .iter()
                    .filter_map(|au| {
                        au.full_name
                            .as_deref()
                            .map(str::trim)
                            .filter(|n| !n.is_empty())
                    })
                    .collect();
                if !names.is_empty() {
                    snippet.push_str(&names.join(", "));
                    snippet.push_str(". ");
                }
            }
            if let Some(date) = a
                .published_date
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(&format!("{date}. "));
            }
            if let Some(t) = a
                .defined_type_name
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                snippet.push_str(&format!("Type: {t}. "));
            }
            if let Some(desc) = a
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

pub struct FigshareProvider {
    config: Config,
}

impl FigshareProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for FigshareProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 100);
        let response = api_client()
            .post(ENDPOINT)
            .json(&flojo_mcp::serde_json::json!({
                "search_for": query,
                "page_size": per_page,
                "page": page,
            }))
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
            .post(ENDPOINT)
            .json(&flojo_mcp::serde_json::json!({ "search_for": "rust", "page_size": 1 }))
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

    const FIXTURE: &str = r#"[
      {
        "id": 999,
        "title": "A Rust dataset",
        "url": "https://figshare.com/articles/999",
        "published_date": "2024-03-01",
        "description": "Benchmark data",
        "defined_type_name": "Dataset",
        "authors": [{ "full_name": "Alice Smith" }]
      },
      { "title": "" }
    ]"#;

    #[test]
    fn parses_articles_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "A Rust dataset");
        assert!(results[0].snippet.contains("Alice Smith."));
        assert!(results[0].snippet.contains("2024-03-01."));
        assert!(results[0].snippet.contains("Type: Dataset."));
        assert!(results[0].snippet.contains("Benchmark data."));
        assert_eq!(results[0].url, "https://figshare.com/articles/999");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
