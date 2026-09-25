//! Public result contracts shared by tools and providers.

use flojo_mcp::schemars::JsonSchema;
use flojo_mcp::serde::{Deserialize, Serialize};

/// Normalized, compact search result (token-efficient payload).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct SearchResult {
    /// Result URL
    pub url: String,
    /// Compact hostname the result points at (no scheme, no path, no `www.`)
    pub source: String,
    /// Page title (truncated)
    pub title: String,
    /// Short snippet, already truncated to keep payloads small
    pub snippet: String,
    /// Engine that produced the result (duckduckgo, bing, brave, ...)
    pub engine: Option<String>,
}

/// Per-provider outcome for one [`search`] call.
///
/// One entry per configured provider (always - even if the engine failed), so
/// the agent sees exactly which engines contributed, which were empty, and
/// which errored. The fanout never silently hides a missing provider.
///
/// `Vec<ProviderEntry>` instead of `BTreeMap` to keep a single `schemars`
/// version in the dep graph (`BTreeMap`'s `JsonSchema` impl comes from a
/// newer `schemars` that `rmcp` pulls in, while our types derive from
/// `flojo_mcp`'s `schemars`).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct SearchOutcome {
    /// Merged organic results, deduplicated by URL.
    pub results: Vec<SearchResult>,
    /// One entry per provider, in fanout order.
    pub providers: Vec<ProviderEntry>,
    /// Human-readable warnings, one per degraded provider.
    pub warnings: Vec<String>,
}

/// `(provider name, status)` row in [`SearchOutcome::providers`].
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct ProviderEntry {
    /// Provider name (duckduckgo, bing, brave, ...).
    pub name: String,
    /// What happened with this provider during the call.
    pub status: ProviderStatus,
}

/// What happened with one provider during a single search call.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderStatus {
    /// Provider returned this many results.
    Ok { count: usize },
    /// Provider answered with HTTP200 but yielded no organic results.
    Empty,
    /// Provider returned an anti-bot rate-limit response.
    RateLimited { status: u16 },
    /// Provider could not be reached (transport error, timeout, ...).
    Unreachable { message: String },
}

/// Engine health and configuration status.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct Status {
    /// Engine name
    pub name: String,
    /// Engine version
    pub version: String,
    /// Configured SearXNG base URL (optional adapter)
    pub searxng_url: String,
    /// Whether the optional SearXNG instance answered a health probe
    pub searxng_reachable: bool,
    /// Configured search providers and their reachability
    pub providers: Vec<ProviderHealth>,
}

/// Reachability report for one configured provider.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct ProviderHealth {
    /// Provider name (duckduckgo, bing, searxng, ...)
    pub name: String,
    /// Whether the provider answered the transport probe
    pub reachable: bool,
}
