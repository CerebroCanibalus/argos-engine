use flojo_mcp::prelude::*;

use crate::config::Config;
use crate::error::ArgosError;
use crate::limits::{clamp_limit, clamp_page};
use crate::providers::SearchProvider;
use crate::providers::searxng::SearxNgProvider;
use crate::types::SearchResult;

#[tool(
    description = "Web search through the local SearXNG instance (70+ engines, no API keys). Returns compact ranked results: url, title, snippet, engine."
)]
pub async fn search(
    query: String,
    limit: Option<usize>,
    page: Option<usize>,
) -> Result<Vec<SearchResult>, ToolError> {
    let query = query.trim();
    if query.is_empty() {
        return Err(ArgosError::InvalidQuery("query must not be empty".into()).into());
    }
    let provider = SearxNgProvider::new(Config::from_env());
    let results = provider
        .search(query, clamp_limit(limit), clamp_page(page))
        .await?;
    Ok(results)
}
