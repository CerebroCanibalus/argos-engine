//! Argos Engine - token-efficient, local-first web research MCP server.
//!
//! Wiring only: tool handlers live in [`tools`], logic in [`providers`],
//! typed failures in [`error`], payload budgets in [`limits`].

mod config;
mod error;
mod limits;
mod providers;
mod tools;
mod types;

use flojo_mcp::prelude::*;

#[flojo_mcp(name = "argos-engine", version = "0.1.0")]
struct ArgosEngine;

#[tokio::main]
async fn main() -> Result<(), ServerError> {
    flojo_run_stdio(ArgosEngine::new()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use flojo_mcp::serde_json::json;

    use crate::types::Status;

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
