//! Lobsters adapter (`news` profile).
//!
//! Public JSON search endpoint, no key. Returns stories with score,
//! comment count and a canonical URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "lobsters";
const ENDPOINT: &str = "https://lobste.rs/search.json";

#[derive(Deserialize, Debug, Default)]
struct RawStory {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    short_id: Option<String>,
    #[serde(default)]
    score: Option<i64>,
    #[serde(default)]
    comment_count: Option<i64>,
    #[serde(default)]
    submitter_user: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: Vec<RawStory> = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .into_iter()
        .filter_map(|story| {
            let title = story
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = story
                .url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    story
                        .short_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(|id| format!("https://lobste.rs/s/{id}"))
                })
                .unwrap_or_else(|| "https://lobste.rs".to_string());
            let mut snippet = String::new();
            if let Some(submitter) = story
                .submitter_user
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                snippet.push_str(&format!("By {submitter}. "));
            }
            if let Some(score) = story.score {
                snippet.push_str(&format!("{score} points. "));
            }
            if let Some(comments) = story.comment_count {
                snippet.push_str(&format!("{comments} comments. "));
            }
            if let Some(tags) = story.tags.as_ref().filter(|t| !t.is_empty()) {
                snippet.push_str(&format!("Tags: {}. ", tags.join(", ")));
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct LobstersProvider {
    config: Config,
}

impl LobstersProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for LobstersProvider {
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
                ("what", "stories".to_string()),
                ("order", "relevance".to_string()),
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
            .query(&[("q", "rust".to_string()), ("what", "stories".to_string())])
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
        "title": "Rust async runtimes compared",
        "url": "https://example.com/async",
        "short_id": "abc123",
        "score": 25,
        "comment_count": 4,
        "submitter_user": "bob",
        "tags": ["rust", "programming"]
      },
      { "title": "" }
    ]"#;

    #[test]
    fn parses_stories_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust async runtimes compared");
        assert!(results[0].snippet.contains("By bob."));
        assert!(results[0].snippet.contains("25 points."));
        assert!(results[0].snippet.contains("4 comments."));
        assert!(results[0].snippet.contains("Tags: rust, programming."));
        assert_eq!(results[0].url, "https://example.com/async");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
