//! Search provider abstraction (M1: local SearXNG; M3: optional cloud adapters).

pub mod searxng;

use flojo_mcp::async_trait::async_trait;

use crate::error::ArgosError;
use crate::types::SearchResult;

/// A source of web results. Implementations must be cancellation-agnostic
/// and fast-failing (bounded by their own per-request timeouts).
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Run a paged search and return normalized, compact results.
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<SearchResult>, ArgosError>;

    /// Cheap reachability probe; `false` when the source is down.
    async fn health(&self) -> bool;
}
