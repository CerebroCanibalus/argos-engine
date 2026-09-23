//! SearXNG JSON API client for the local instance (no keys, no accounts).

use std::time::Duration;

use flojo_mcp::prelude::*;
use flojo_mcp::serde::Deserialize;
use flojo_mcp::truncate_string;

use crate::SearchResult;

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8080";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const PING_TIMEOUT: Duration = Duration::from_secs(2);
const TITLE_MAX_CHARS: usize = 200;
const SNIPPET_MAX_CHARS: usize = 300;

/// Base URL of the local SearXNG instance. Env `ARGOS_SEARXNG_URL` overrides the default.
pub fn base_url() -> String {
    std::env::var("ARGOS_SEARXNG_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
}

/// Probe the instance root; false when unreachable (no Docker / not started).
pub async fn ping(base: &str) -> bool {
    let client = match reqwest::Client::builder().timeout(PING_TIMEOUT).build() {
        Ok(client) => client,
        Err(_) => return false,
    };
    match client.get(base).send().await {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    }
}

/// One result as returned by the SearXNG JSON API.
#[derive(Deserialize, Debug)]
struct RawResult {
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    engine: Option<String>,
}

/// SearXNG `/search` JSON payload (unknown fields are ignored).
#[derive(Deserialize, Debug)]
struct RawResponse {
    #[serde(default)]
    results: Vec<RawResult>,
}

/// Run a search against the local SearXNG JSON API and normalize the results.
pub async fn search(
    query: &str,
    limit: usize,
    page: usize,
) -> Result<Vec<SearchResult>, ToolError> {
    let base = base_url();
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|ex| ToolError::internal(format!("failed to build HTTP client: {ex}")))?;

    let response = client
        .get(format!("{base}/search"))
        .query(&[
            ("q", query.to_string()),
            ("format", "json".to_string()),
            ("page", page.to_string()),
            ("safesearch", "0".to_string()),
        ])
        .send()
        .await
        .map_err(|ex| {
            ToolError::internal(format!(
                "SearXNG unreachable at {base}: {ex}. Start it with: docker compose -f searxng/docker-compose.yml up -d"
            ))
        })?;

    if !response.status().is_success() {
        let hint = flojo_mcp::serde_json::json!({
            "hint": "JSON API disabled? Ensure searxng/settings.yml sets search.formats: [html, json] and restart the container.",
        });
        return Err(
            ToolError::internal(format!("SearXNG returned HTTP {}", response.status()))
                .with_data(hint),
        );
    }

    let payload: RawResponse = response
        .json()
        .await
        .map_err(|ex| ToolError::internal(format!("SearXNG sent an unexpected payload: {ex}")))?;

    Ok(payload
        .results
        .into_iter()
        .take(limit)
        .map(|raw| SearchResult {
            url: raw.url,
            title: truncate_string(&raw.title, TITLE_MAX_CHARS).0,
            snippet: truncate_string(&raw.content, SNIPPET_MAX_CHARS).0,
            engine: raw.engine,
        })
        .collect())
}
