//! Reddit adapter (`news`/`general` profiles).
//!
//! Public search JSON endpoint, no key (requires a descriptive User-Agent,
//! which the shared api_client already sends). Returns posts with score,
//! comment count and a canonical permalink.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "reddit";
const ENDPOINT: &str = "https://www.reddit.com/search.json";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    data: RawData,
}

#[derive(Deserialize, Debug, Default)]
struct RawData {
    #[serde(default)]
    children: Vec<RawChild>,
}

#[derive(Deserialize, Debug, Default)]
struct RawChild {
    #[serde(default)]
    data: RawPost,
}

#[derive(Deserialize, Debug, Default)]
struct RawPost {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    permalink: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    selftext: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    subreddit: Option<String>,
    #[serde(default)]
    score: Option<i64>,
    #[serde(default, rename = "num_comments")]
    num_comments: Option<i64>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .data
        .children
        .into_iter()
        .filter_map(|child| {
            let post = child.data;
            let title = post
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = post
                .permalink
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(|p| format!("https://www.reddit.com{p}"))
                .or_else(|| {
                    post.url
                        .as_deref()
                        .map(str::trim)
                        .filter(|u| !u.is_empty())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "https://www.reddit.com".to_string());
            let mut snippet = String::new();
            if let Some(subreddit) = post
                .subreddit
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                snippet.push_str(&format!("r/{subreddit}. "));
            }
            if let Some(author) = post
                .author
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())
            {
                snippet.push_str(&format!("By u/{author}. "));
            }
            if let Some(score) = post.score {
                snippet.push_str(&format!("{score} points. "));
            }
            if let Some(comments) = post.num_comments {
                snippet.push_str(&format!("{comments} comments. "));
            }
            if let Some(text) = post
                .selftext
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                snippet.push_str(text);
                snippet.push_str(". ");
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct RedditProvider {
    config: Config,
}

impl RedditProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for RedditProvider {
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
                ("limit", per_page.to_string()),
                ("sort", "relevance".to_string()),
                ("t", "all".to_string()),
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
        if page > 1 {
            let skip = ((page.saturating_sub(1)) * per_page).min(results.len());
            results.drain(..skip);
        }
        results.truncate(limit);
        Ok(results)
    }

    async fn health(&self) -> bool {
        match api_client()
            .get(ENDPOINT)
            .query(&[("q", "rust".to_string()), ("limit", "1".to_string())])
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
      "data": {
        "children": [
          {
            "data": {
              "title": "Rust 2024 edition impressions",
              "permalink": "/r/rust/comments/abc/rust_2024/",
              "url": "https://example.com/post",
              "selftext": "First impressions after upgrading",
              "author": "carol",
              "subreddit": "rust",
              "score": 120,
              "num_comments": 30
            }
          },
          { "data": { "title": "" } }
        ]
      }
    }"#;

    #[test]
    fn parses_posts_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust 2024 edition impressions");
        assert!(results[0].snippet.contains("r/rust."));
        assert!(results[0].snippet.contains("By u/carol."));
        assert!(results[0].snippet.contains("120 points."));
        assert!(results[0].snippet.contains("30 comments."));
        assert!(
            results[0]
                .snippet
                .contains("First impressions after upgrading.")
        );
        assert_eq!(
            results[0].url,
            "https://www.reddit.com/r/rust/comments/abc/rust_2024/"
        );
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
