//! Public result contracts shared by tools and providers.

use flojo_mcp::schemars::JsonSchema;
use flojo_mcp::serde::{Deserialize, Serialize};

/// Normalized, compact search result (token-efficient payload).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct SearchResult {
    /// Result URL
    pub url: String,
    /// Page title (truncated)
    pub title: String,
    /// Short snippet, already truncated to keep payloads small
    pub snippet: String,
    /// SearXNG engine that produced the result, when reported
    pub engine: Option<String>,
}

/// Engine health and configuration status.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct Status {
    /// Engine name
    pub name: String,
    /// Engine version
    pub version: String,
    /// Configured SearXNG base URL
    pub searxng_url: String,
    /// Whether the local SearXNG instance answered a health probe
    pub searxng_reachable: bool,
}
