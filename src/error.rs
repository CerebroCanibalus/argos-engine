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

    /// The instance exists but nothing is listening (connection refused).
    #[error("SearXNG is not running at {base}")]
    Down {
        /// Configured base URL.
        base: String,
    },

    /// The configured SearXNG instance did not answer within the timeout.
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

    /// The on-demand stack boot failed before the budget elapsed.
    #[error("SearXNG stack failed to start: {detail}")]
    StackBoot {
        /// Why the boot attempt failed.
        detail: String,
    },

    /// The stack is still booting after the configured budget.
    #[error("SearXNG stack is still starting after {secs}s")]
    StackStarting {
        /// Seconds waited so far.
        secs: u64,
    },
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
            ArgosError::InvalidQuery(_) => tool_error,
            ArgosError::Down { .. } => tool_error.with_data(json!({
                "hint": "Auto-start is enabled by default (ARGOS_AUTO_START=1). If the one-time provisioning is missing, run searxng\\wsl-setup.bat once as admin; afterwards searxng\\wsl-up.bat starts the stack manually.",
            })),
            ArgosError::Unreachable { .. } => tool_error.with_data(json!({
                "hint": "Instance configured but not answering. Check ARGOS_SEARXNG_URL and the stack logs: searxng\\wsl-up.bat, then docker-compose logs.",
            })),
            ArgosError::Http { hint, .. } => tool_error.with_data(json!({ "hint": hint })),
            ArgosError::StackBoot { .. } => tool_error.with_data(json!({
                "hint": "Run searxng\\wsl-setup.bat once as admin to provision Ubuntu + Docker Engine, or start manually with searxng\\wsl-up.bat to see the full output.",
            })),
            ArgosError::StackStarting { .. } => tool_error.with_data(json!({
                "hint": "Boot continues in the background; retry the search in a few seconds.",
            })),
            ArgosError::Decode(_) => tool_error,
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
    fn down_carries_provisioning_hint() {
        let error: ToolError = ArgosError::Down {
            base: "http://127.0.0.1:8080".into(),
        }
        .into();
        let data = error.to_error_data();
        assert!(data.message.contains("not running"));
        let hint = data.data.expect("hint payload expected")["hint"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(hint.contains("wsl-setup.bat"));
        assert!(hint.contains("ARGOS_AUTO_START"));
    }

    #[test]
    fn unreachable_carries_actionable_hint() {
        let error: ToolError = ArgosError::Unreachable {
            base: "http://127.0.0.1:8080".into(),
            cause: "operation timed out".into(),
        }
        .into();
        let data = error.to_error_data();
        assert!(data.message.contains("SearXNG unreachable"));
        let hint = data.data.expect("hint payload expected")["hint"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(hint.contains("ARGOS_SEARXNG_URL"));
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

    #[test]
    fn stack_starting_suggests_retry() {
        let error: ToolError = ArgosError::StackStarting { secs: 60 }.into();
        let data = error.to_error_data();
        assert!(data.message.contains("still starting"));
        let hint = data.data.expect("hint payload expected")["hint"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(hint.contains("retry"));
    }
}
