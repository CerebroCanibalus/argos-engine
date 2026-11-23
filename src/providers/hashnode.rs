//! Hashnode adapter (`knowledge` profile).
//!
//! Public GraphQL API, no key. Returns posts matching a search term with
//! title, brief and a canonical hashnode.com URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "hashnode";
const ENDPOINT: &str = "https://gql.hashnode.com";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    data: Option<RawData>,
}

#[derive(Deserialize, Debug, Default)]
struct RawData {
    #[serde(default, rename = "searchPosts")]
    search_posts: Option<RawSearchPosts>,
}

#[derive(Deserialize, Debug, Default)]
struct RawSearchPosts {
    #[serde(default, rename = "edges")]
    edges: Vec<RawEdge>,
}

#[derive(Deserialize, Debug, Default)]
struct RawEdge {
    #[serde(default)]
    node: Option<RawNode>,
}

#[derive(Deserialize, Debug, Default)]
struct RawNode {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    brief: Option<String>,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    author: Option<RawAuthor>,
}

#[derive(Deserialize, Debug, Default)]
struct RawAuthor {
    #[serde(default)]
    username: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    let edges = parsed
        .data
        .and_then(|d| d.search_posts)
        .map(|s| s.edges)
        .unwrap_or_default();
    Ok(edges
        .into_iter()
        .filter_map(|e| {
            let node = e.node.unwrap_or_default();
            let title = node.title.as_deref().map(str::trim).filter(|t| !t.is_empty())?;
            let url = node
                .url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    node.slug
                        .as_deref()
                        .map(|s| format!("https://hashnode.com/post/{s}"))
                        .unwrap_or_else(|| "https://hashnode.com".to_string())
                });
            let mut snippet = String::new();
            if let Some(brief) = node.brief.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
                snippet.push_str(brief);
                snippet.push_str(". ");
            }
            if let Some(author) = node.author.as_ref().and_then(|a| a.username.as_deref()).map(str::trim).filter(|a| !a.is_empty()) {
                snippet.push_str(&format!("By {author}. "));
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct HashnodeProvider {
    config: Config,
}

impl HashnodeProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for HashnodeProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 50);
        let body = flojo_mcp::serde_json::json!({
            "query": "query SearchPosts($q: String!, $first: Int!) { searchPosts(input: { query: $q, first: $first, sortBy: RELEVANCE }) { edges { node { title brief slug url author { username } } } } }",
            "variables": { "q": query, "first": per_page }
        });
        let _ = page;
        let response = api_client()
            .post(ENDPOINT)
            .json(&body)
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
        let body = flojo_mcp::serde_json::json!({
            "query": "query { __typename }"
        });
        match api_client()
            .post(ENDPOINT)
            .json(&body)
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
        "searchPosts": {
          "edges": [
            {
              "node": {
                "title": "Building a Rust async runtime",
                "brief": "A walkthrough of tokio",
                "slug": "rust-async-runtime",
                "url": "https://hashnode.com/post/rust-async-runtime",
                "author": { "username": "alice" }
              }
            },
            { "node": { "title": "" } }
          ]
        }
      }
    }"#;

    #[test]
    fn parses_posts_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Building a Rust async runtime");
        assert!(results[0].snippet.contains("A walkthrough of tokio."));
        assert!(results[0].snippet.contains("By alice."));
        assert_eq!(results[0].url, "https://hashnode.com/post/rust-async-runtime");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
