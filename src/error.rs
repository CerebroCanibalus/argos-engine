//! Typed errors mapped to MCP `ToolError` with actionable hints.
//!
//! Anti-goal (inherited from the Python reference MCP): never return a generic
//! "Error executing tool" ghost - every failure carries a real message and,
//! when useful, an operator hint in the error data.

use flojo_mcp::prelude::*;
use flojo_mcp::serde_json::json;

/// All failures that can surface from Argos Engine tools.
#[derive(Debug, thiserror::Error)]
pub enum ArgosError {
    /// Caller supplied an unusable query.
    #[error("invalid query: {0}")]
    InvalidQuery(String),

    /// The configured SearXNG instance did not answer.
    #[error("SearXNG unreachable at {base}: {cause}")]
    Unreachable {
        /// Configured base URL.
        base: String,
        /// Underlying transport error.
        cause: String,
    },

    /// SearXNG answered with a non-200 status.
    #[error("SearXNG returned HTTP {status}")]
    Http {
        /// HTTP status code.
        status: u16,
        /// Actionable hint for the operator.
        hint: &'static str,
    },

    /// The payload did not match the SearXNG JSON contract.
    #[error("unexpected SearXNG payload: {0}")]
    Decode(String),
}

impl From<ArgosError> for ToolError {
    fn from(error: ArgosError) -> Self {
        let is_invalid_query = matches!(&error, ArgosError::InvalidQuery(_));
        let tool_error = if is_invalid_query {
            ToolError::invalid_params(error.to_string())
        } else {
            ToolError::internal(error.to_string())
        };
        match &error {
            ArgosError::Unreachable { base, .. } => tool_error.with_data(json!({
                "hint": format!(
                    "Start the instance: docker compose -f searxng/docker-compose.yml up -d ({base})"
                ),
            })),
            ArgosError::Http { hint, .. } => tool_error.with_data(json!({ "hint": hint })),
            _ => tool_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_query_maps_to_invalid_params() {
        let error: ToolError = ArgosError::InvalidQuery("query must not be empty".into()).into();
        let data = error.to_error_data();
        assert_eq!(data.message, "invalid query: query must not be empty");
    }

    #[test]
    fn unreachable_carries_actionable_hint() {
        let error: ToolError = ArgosError::Unreachable {
            base: "http://127.0.0.1:8080".into(),
            cause: "connection refused".into(),
        }
        .into();
        let data = error.to_error_data();
        assert!(data.message.contains("SearXNG unreachable"));
        let hint = data.data.expect("hint payload expected");
        assert!(
            hint["hint"]
                .as_str()
                .unwrap_or_default()
                .contains("docker compose")
        );
    }

    #[test]
    fn http_error_carries_operator_hint() {
        let error: ToolError = ArgosError::Http {
            status: 403,
            hint: "check settings",
        }
        .into();
        let data = error.to_error_data();
        assert!(data.message.contains("403"));
        assert_eq!(data.data.expect("hint payload")["hint"], "check settings");
    }
}
