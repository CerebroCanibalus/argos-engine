//! Hacker News adapter (`news` profile).
//!
//! Algolia-powered search API, no key, returns stories and comments with
//! points and a canonical HN item URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "hackernews";
const ENDPOINT: &str = "https://hn.algolia.com/api/v1/search";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    hits: Vec<RawHit>,
}

#[derive(Deserialize, Debug, Default)]
struct RawHit {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    story_title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    story_url: Option<String>,
    #[serde(default)]
    object_id: Option<String>,
    #[serde(default)]
    points: Option<i64>,
    #[serde(default)]
    num_comments: Option<i64>,
    #[serde(default)]
    author: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .hits
        .into_iter()
        .filter_map(|hit| {
            let title = hit
                .title
                .as_deref()
                .or(hit.story_title.as_deref())
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = hit
                .url
                .as_deref()
                .or(hit.story_url.as_deref())
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    hit.object_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(|id| format!("https://news.ycombinator.com/item?id={id}"))
                })
                .unwrap_or_else(|| "https://news.ycombinator.com".to_string());
            let mut snippet = String::new();
            if let Some(author) = hit
                .author
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())
            {
                snippet.push_str(&format!("By {author}. "));
            }
            if let Some(points) = hit.points {
                snippet.push_str(&format!("{points} points. "));
            }
            if let Some(comments) = hit.num_comments {
                snippet.push_str(&format!("{comments} comments. "));
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct HackerNewsProvider {
    config: Config,
}

impl HackerNewsProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for HackerNewsProvider {
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
                ("hitsPerPage", per_page.to_string()),
                ("page", page.saturating_sub(1).to_string()),
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
                ("hitsPerPage", "1".to_string()),
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
      "hits": [
        {
          "title": "Show HN: A Rust async runtime",
          "url": "https://example.com/runtime",
          "objectID": "123",
          "points": 42,
          "num_comments": 7,
          "author": "alice"
        },
        { "title": "", "objectID": "999" }
      ]
    }"#;

    #[test]
    fn parses_stories_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Show HN: A Rust async runtime");
        assert!(results[0].snippet.contains("By alice."));
        assert!(results[0].snippet.contains("42 points."));
        assert!(results[0].snippet.contains("7 comments."));
        assert_eq!(results[0].url, "https://example.com/runtime");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
