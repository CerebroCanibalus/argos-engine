//! Stack Overflow adapter (`code`/`knowledge` profiles).
//!
//! Public Stack Exchange search endpoint, no key (300 req/day anonymous).
//! Returns questions with score, answer count and a canonical question URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "stackoverflow";
const ENDPOINT: &str = "https://api.stackexchange.com/2.3/search";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    items: Vec<RawItem>,
}

#[derive(Deserialize, Debug, Default)]
struct RawItem {
    #[serde(default)]
    title: Option<String>,
    #[serde(default, rename = "link")]
    link: Option<String>,
    #[serde(default)]
    score: Option<i64>,
    #[serde(default, rename = "answer_count")]
    answer_count: Option<i64>,
    #[serde(default, rename = "view_count")]
    view_count: Option<i64>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default, rename = "is_answered")]
    is_answered: Option<bool>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .items
        .into_iter()
        .filter_map(|item| {
            let title = item
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = item
                .link
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| "https://stackoverflow.com".to_string());
            let mut snippet = String::new();
            if let Some(score) = item.score {
                snippet.push_str(&format!("{score} votes. "));
            }
            if let Some(answers) = item.answer_count {
                snippet.push_str(&format!("{answers} answers. "));
            }
            if let Some(views) = item.view_count {
                snippet.push_str(&format!("{views} views. "));
            }
            if item.is_answered == Some(true) {
                snippet.push_str("Answered. ");
            }
            if let Some(tags) = item.tags.as_ref().filter(|t| !t.is_empty()) {
                snippet.push_str(&format!("Tags: {}. ", tags.join(", ")));
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct StackOverflowProvider {
    config: Config,
}

impl StackOverflowProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for StackOverflowProvider {
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
                ("order", "desc".to_string()),
                ("sort", "relevance".to_string()),
                ("intitle", query.to_string()),
                ("site", "stackoverflow".to_string()),
                ("pagesize", per_page.to_string()),
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
                ("order", "desc".to_string()),
                ("sort", "relevance".to_string()),
                ("intitle", "rust".to_string()),
                ("site", "stackoverflow".to_string()),
                ("pagesize", "1".to_string()),
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
      "items": [
        {
          "title": "How do I borrow a value in Rust?",
          "link": "https://stackoverflow.com/questions/123/how-do-i-borrow",
          "score": 100,
          "answer_count": 5,
          "view_count": 2000,
          "tags": ["rust", "borrow-checker"],
          "is_answered": true
        },
        { "title": "" }
      ]
    }"#;

    #[test]
    fn parses_questions_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "How do I borrow a value in Rust?");
        assert!(results[0].snippet.contains("100 votes."));
        assert!(results[0].snippet.contains("5 answers."));
        assert!(results[0].snippet.contains("2000 views."));
        assert!(results[0].snippet.contains("Answered."));
        assert!(results[0].snippet.contains("Tags: rust, borrow-checker."));
        assert_eq!(
            results[0].url,
            "https://stackoverflow.com/questions/123/how-do-i-borrow"
        );
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
