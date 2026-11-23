//! dev.to adapter (`news`/`code` profiles).
//!
//! Public articles API, no key. Returns title, description, tags and a
//! canonical article URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "devto";
const ENDPOINT: &str = "https://dev.to/api/articles";

#[derive(Deserialize, Debug, Default)]
struct RawArticle {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    tag_list: Option<Vec<String>>,
    #[serde(default)]
    user: Option<RawUser>,
    #[serde(default)]
    positive_reactions_count: Option<i64>,
    #[serde(default)]
    comments_count: Option<i64>,
}

#[derive(Deserialize, Debug, Default)]
struct RawUser {
    #[serde(default)]
    name: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: Vec<RawArticle> = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .into_iter()
        .filter_map(|article| {
            let title = article
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = article
                .url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| "https://dev.to".to_string());
            let mut snippet = String::new();
            if let Some(description) = article
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(description);
                snippet.push_str(". ");
            }
            if let Some(user) = article
                .user
                .as_ref()
                .and_then(|u| u.name.as_deref())
                .map(str::trim)
                .filter(|n| !n.is_empty())
            {
                snippet.push_str(&format!("By {user}. "));
            }
            if let Some(reactions) = article.positive_reactions_count {
                snippet.push_str(&format!("{reactions} reactions. "));
            }
            if let Some(comments) = article.comments_count {
                snippet.push_str(&format!("{comments} comments. "));
            }
            if let Some(tags) = article.tag_list.as_ref().filter(|t| !t.is_empty()) {
                snippet.push_str(&format!("Tags: {}. ", tags.join(", ")));
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct DevtoProvider {
    config: Config,
}

impl DevtoProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for DevtoProvider {
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
                ("per_page", per_page.to_string()),
                ("page", page.to_string()),
            ])
            .timeout(self.config.request_timeout)
            .send()
            .await
            .map_err(|ex| ArgosError::Unreachable {
                origin: ENGINE.into(),
                cause: ex.to_string(),
            })?;

        // dev.to has no free-text query param in this endpoint; filter client-side
        // by dropping results whose title/description do not contain the query.
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
        let needle = query.to_lowercase();
        results.retain(|r| {
            r.title.to_lowercase().contains(&needle) || r.snippet.to_lowercase().contains(&needle)
        });
        results.truncate(limit);
        Ok(results)
    }

    async fn health(&self) -> bool {
        match api_client()
            .get(ENDPOINT)
            .query(&[("per_page", "1".to_string())])
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
        "title": "Building a Rust async runtime",
        "description": "A walkthrough of tokio internals",
        "url": "https://dev.to/alice/rust-async",
        "tag_list": ["rust", "async"],
        "user": { "name": "Alice" },
        "positive_reactions_count": 42,
        "comments_count": 7
      },
      { "title": "" }
    ]"#;

    #[test]
    fn parses_articles_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Building a Rust async runtime");
        assert!(
            results[0]
                .snippet
                .contains("A walkthrough of tokio internals.")
        );
        assert!(results[0].snippet.contains("By Alice."));
        assert!(results[0].snippet.contains("42 reactions."));
        assert!(results[0].snippet.contains("7 comments."));
        assert!(results[0].snippet.contains("Tags: rust, async."));
        assert_eq!(results[0].url, "https://dev.to/alice/rust-async");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
