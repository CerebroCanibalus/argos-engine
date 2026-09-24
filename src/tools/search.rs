use flojo_mcp::prelude::*;

use crate::config::Config;
use crate::error::ArgosError;
use crate::limits::{clamp_limit, clamp_page};
use crate::providers::SearchProvider;
use crate::providers::fanout::Fanout;
use crate::types::SearchResult;

#[tool(
    description = "Web search through keyless providers (DuckDuckGo + Bing + Brave by default) with parallel fanout, URL dedup and automatic failover. Compact results: url, title, snippet, engine."
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
    let fanout = Fanout::from_config(&config);
    let results = fanout
        .search(&query, clamp_limit(limit), clamp_page(page))
        .await?;
    Ok(results)
}
