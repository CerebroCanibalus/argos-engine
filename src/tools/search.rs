use flojo_mcp::prelude::*;

use crate::config::Config;
use crate::error::ArgosError;
use crate::limits::{clamp_limit, clamp_page};
use crate::providers::fanout::Fanout;
use crate::types::SearchOutcome;

#[tool(
    description = "Web search through keyless providers (DuckDuckGo + Bing + Brave by default) with parallel fanout, URL dedup and automatic failover. Returns {results, providers (per-engine status), warnings} so the agent sees which engines contributed, which were empty, and which errored - the fanout never silently hides a missing provider. Compact results: url, source (hostname), title, snippet, engine. Optional `domains` argument site-scopes the query to specific hosts (e.g. ['kvrforums.com','reddit.com']) - critical for niche research where Bing's general index misreads ambiguous tokens."
)]
pub async fn search(
    query: String,
    limit: Option<usize>,
    page: Option<usize>,
    domains: Option<Vec<String>>,
) -> Result<SearchOutcome, ToolError> {
    let mut query = query.trim().to_string();
    if query.is_empty() {
        return Err(ArgosError::InvalidQuery("query must not be empty".into()).into());
    }

    // Append `site:host` filters when `domains` is supplied. The `site:`
    // operator is supported by all three default providers, so we don't
    // need to special-case per engine.
    let sites: Vec<String> = domains
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|host| format!("site:{host}"))
        .collect();
    if !sites.is_empty() {
        query.push_str(&format!(" ({})", sites.join(" OR ")));
    }

    let config = Config::from_env();
    let fanout = Fanout::from_config(&config);
    let outcome = fanout
        .run(&query, clamp_limit(limit), clamp_page(page))
        .await?;
    Ok(outcome)
}
