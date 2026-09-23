use flojo_mcp::prelude::*;

use crate::config::Config;
use crate::error::ArgosError;
use crate::limits::{clamp_limit, clamp_page};
use crate::providers::SearchProvider;
use crate::providers::searxng::SearxNgProvider;
use crate::stack;
use crate::types::SearchResult;

#[tool(
    description = "Web search through the local SearXNG instance (70+ engines, no API keys). Boots the stack on demand and stops it after idle. Returns compact ranked results: url, title, snippet, engine."
)]
pub async fn search(
    query: String,
    limit: Option<usize>,
    page: Option<usize>,
) -> Result<Vec<SearchResult>, ToolError> {
    let query = query.trim().to_string();
    if query.is_empty() {
        return Err(ArgosError::InvalidQuery("query must not be empty".into()).into());
    }

    let config = Config::from_env();
    let provider = SearxNgProvider::new(config.clone());
    let limit = clamp_limit(limit);
    let page = clamp_page(page);

    let mut outcome = provider.search(&query, limit, page).await;

    // Connection refused -> stack is down: boot on demand and retry once.
    // Already running: no probe, no subprocess, no overhead.
    if matches!(&outcome, Err(ArgosError::Down { .. })) && config.auto_start {
        stack::ensure_up(&config).await?;
        outcome = provider.search(&query, limit, page).await;
    }

    let results = outcome?;
    stack::note_usage(&config).await;
    Ok(results)
}
