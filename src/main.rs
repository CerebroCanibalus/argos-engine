//! Argos Engine - token-efficient, local-first web research MCP server.

mod searxng;

use flojo_mcp::prelude::*;
use flojo_mcp::schemars::JsonSchema;
use flojo_mcp::serde::{Deserialize, Serialize};

#[flojo_mcp(name = "argos-engine", version = "0.1.0")]
struct ArgosEngine;

/// Normalized, compact search result (token-efficient payload).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
struct SearchResult {
    /// Result URL
    url: String,
    /// Page title (truncated)
    title: String,
    /// Short snippet, already truncated to keep payloads small
    snippet: String,
    /// SearXNG engine that produced the result, when reported
    engine: Option<String>,
}

/// Engine health and configuration status.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
struct Status {
    /// Engine name
    name: String,
    /// Engine version
    version: String,
    /// Configured SearXNG base URL
    searxng_url: String,
    /// Whether the local SearXNG instance answered a health probe
    searxng_reachable: bool,
}

#[tool(description = "Engine status: version, configuration and local SearXNG reachability")]
async fn status() -> Result<Status, ToolError> {
    let searxng_url = searxng::base_url();
    let searxng_reachable = searxng::ping(&searxng_url).await;
    Ok(Status {
        name: "argos-engine".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        searxng_url,
        searxng_reachable,
    })
}

#[tool(
    description = "Web search through the local SearXNG instance (70+ engines, no API keys). Returns compact ranked results: url, title, snippet, engine."
)]
async fn search(
    query: String,
    limit: Option<usize>,
    page: Option<usize>,
) -> Result<Vec<SearchResult>, ToolError> {
    let query = query.trim();
    if query.is_empty() {
        return Err(ToolError::invalid_params("query must not be empty"));
    }
    let limit = limit.unwrap_or(10).clamp(1, 50);
    let page = page.unwrap_or(1).max(1);
    searxng::search(query, limit, page).await
}

#[tokio::main]
async fn main() -> Result<(), ServerError> {
    flojo_run_stdio(ArgosEngine::new()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use flojo_mcp::serde_json::json;

    #[tokio::test]
    async fn status_reports_engine_identity() {
        let tester = FlojoTester::new(ArgosEngine::new());
        let status: Status = tester.call_typed("status", json!({})).await.unwrap();
        assert_eq!(status.name, "argos-engine");
        assert!(!status.version.is_empty());
        assert!(status.searxng_url.starts_with("http"));
    }

    #[tokio::test]
    async fn search_rejects_empty_query() {
        let tester = FlojoTester::new(ArgosEngine::new());
        let result = tester.call("search", json!({"query": "   "})).await;
        assert!(result.is_err(), "empty query must be rejected");
    }

    #[tokio::test]
    async fn expected_tools_are_listed() {
        let tester = FlojoTester::new(ArgosEngine::new());
        let tools = tester.list_tools().await.unwrap();
        let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
        assert!(
            names.iter().any(|n| n == "search"),
            "search tool missing: {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "status"),
            "status tool missing: {names:?}"
        );
    }
}
