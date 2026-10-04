//! Typed errors mapped to MCP `ToolError` with actionable hints.
//!
//! Anti-goal (inherited from the Python reference MCP): never return a generic
//! "Error executing tool" ghost - every failure carries a real message and,
//! when useful, an operator hint in the error data.

use flojo_mcp::prelude::*;
use flojo_mcp::serde_json::json;

/// All failures that can surface from Argos Engine tools.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ArgosError {
    /// Caller supplied an unusable query.
    #[error("invalid query: {0}")]
    InvalidQuery(String),

    /// A provider did not answer within the timeout or transport failed.
    #[error("{origin} unreachable: {cause}")]
    Unreachable {
        /// Engine name / endpoint identity.
        origin: String,
        /// Underlying transport error.
        cause: String,
    },

    /// The provider answered with an anti-bot rate-limit status.
    #[error("{engine} rate-limited this IP (HTTP {status})")]
    RateLimited {
        /// Engine that rate-limited us.
        engine: String,
        /// HTTP status (202/403/429 observed in the wild).
        status: u16,
        /// Delay the server asked for, when it advertised one. Overrides the
        /// conservative local cooldown so Argos waits exactly as long as asked.
        retry_after_secs: Option<u64>,
    },

    /// Every configured provider rate-limited this IP, so the search returned
    /// nothing because of anti-bot pressure - not because the topic has no hits.
    #[error("all providers rate-limited this IP")]
    AllProvidersRateLimited {
        /// `(engine, http_status, retry_after_secs)` for every rate-limited provider.
        providers: Vec<(String, u16, Option<u64>)>,
    },

    /// Providers answered, but every result was rejected by the requested
    /// domain or lexical relevance gate.
    #[error("providers returned no usable results after quality filtering")]
    NoUsableResults {
        /// Number of raw organic results received before filtering.
        returned: usize,
        /// Number of raw results rejected by the quality gates.
        rejected: usize,
    },

    /// No configured provider is currently eligible because all are in
    /// cooldown, exhausted, degraded, or missing authentication.
    #[error("no search providers are currently eligible")]
    NoEligibleProviders {
        /// Configured provider IDs, for diagnosis.
        providers: Vec<String>,
    },

    /// The payload did not match the expected contract.
    #[error("unexpected payload: {0}")]
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
            ArgosError::InvalidQuery(_) => tool_error,
            ArgosError::Unreachable { .. } => tool_error.with_data(json!({
                "hint": "Network problem with this provider. Providers are independent: the fanout keeps going if another one answers - check ARGOS_PROVIDERS.",
            })),
            ArgosError::RateLimited {
                retry_after_secs, ..
            } => tool_error.with_data(json!({
                "hint": "This engine flagged your IP (202/403/429 are the classic anti-bot answers). Wait it out, and raise the ceiling by enabling more providers via ARGOS_PROVIDERS - the fanout spreads load across independent rate budgets.",
                "retry_after_secs": retry_after_secs,
            })),
            ArgosError::AllProvidersRateLimited { providers } => tool_error.with_data(json!({
                "hint": "Every configured provider answered with an anti-bot status. The search returned nothing because your IP was throttled, not because the topic has no hits.",
                "providers": providers.iter().map(|(name, status, retry_after_secs)| json!({
                    "name": name,
                    "status": status,
                    "retry_after_secs": retry_after_secs,
                })).collect::<Vec<_>>(),
            })),
            ArgosError::NoUsableResults { returned, rejected } => tool_error.with_data(json!({
                "hint": "Providers answered, but no result passed Argos' domain or lexical relevance gate. Inspect the query, narrow the domains, or use another research formulation.",
                "returned": returned,
                "rejected": rejected,
            })),
            ArgosError::NoEligibleProviders { providers } => tool_error.with_data(json!({
                "hint": "All configured providers are temporarily unavailable. Argos will retry them after their cooldown; inspect provider status or add another configured provider.",
                "providers": providers,
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
    fn unreachable_names_the_provider_and_mentions_failover() {
        let error: ToolError = ArgosError::Unreachable {
            origin: "html.duckduckgo.com".into(),
            cause: "operation timed out".into(),
        }
        .into();
        let data = error.to_error_data();
        assert!(data.message.contains("html.duckduckgo.com"));
        assert!(data.message.contains("unreachable"));
        let hint = data.data.expect("hint payload expected")["hint"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(hint.contains("ARGOS_PROVIDERS"));
    }

    #[test]
    fn rate_limit_explains_the_ceiling_lever_and_echoes_retry_after() {
        let error: ToolError = ArgosError::RateLimited {
            engine: "duckduckgo".into(),
            status: 202,
            retry_after_secs: Some(7),
        }
        .into();
        let data = error.to_error_data();
        assert!(data.message.contains("rate-limited"));
        assert!(data.message.contains("202"));
        let payload = data.data.expect("hint payload expected");
        let hint = payload["hint"].as_str().unwrap_or_default();
        assert!(hint.contains("ARGOS_PROVIDERS"));
        assert!(hint.contains("fanout"));
        assert_eq!(payload["retry_after_secs"], 7);
    }

    #[test]
    fn all_providers_rate_limited_lists_them_and_distinguishes_from_no_results() {
        let error: ToolError = ArgosError::AllProvidersRateLimited {
            providers: vec![
                ("duckduckgo".into(), 202, None),
                ("bing".into(), 429, Some(11)),
            ],
        }
        .into();
        let data = error.to_error_data();
        assert!(data.message.contains("all providers rate-limited"));
        let payload = data.data.expect("hint payload expected");
        let hint = payload["hint"].as_str().unwrap_or_default();
        assert!(
            hint.contains("throttled"),
            "hint must explain this isn't 'no hits': {hint}"
        );
        let listed = payload["providers"].as_array().expect("providers array");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[1]["retry_after_secs"], 11);
    }

    #[test]
    fn no_usable_results_explains_quality_filtering() {
        let error: ToolError = ArgosError::NoUsableResults {
            returned: 10,
            rejected: 10,
        }
        .into();
        let data = error.to_error_data();
        assert!(data.message.contains("no usable results"));
        let payload = data.data.expect("quality payload");
        assert_eq!(payload["returned"], 10);
        assert_eq!(payload["rejected"], 10);
        assert!(
            payload["hint"]
                .as_str()
                .unwrap_or_default()
                .contains("domain")
        );
    }

    #[test]
    fn no_eligible_providers_explains_cooldown_state() {
        let error: ToolError = ArgosError::NoEligibleProviders {
            providers: vec!["brave".into(), "ddg".into()],
        }
        .into();
        let data = error.to_error_data();
        let payload = data.data.expect("provider payload");
        assert!(data.message.contains("no search providers"));
        assert_eq!(payload["providers"].as_array().expect("providers").len(), 2);
        assert!(
            payload["hint"]
                .as_str()
                .unwrap_or_default()
                .contains("cooldown")
        );
    }
}
