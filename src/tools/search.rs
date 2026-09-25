use flojo_mcp::prelude::*;

use crate::config::Config;
use crate::error::ArgosError;
use crate::limits::{clamp_limit, clamp_page};
use crate::metasearch::MetasearchRouter;
use crate::quality;
use crate::types::SearchOutcome;

#[tool(
    description = "Web search through keyless providers (DuckDuckGo + Bing + Brave by default) with parallel fanout, URL dedup and automatic failover. Returns {results, providers (per-engine status), warnings} so the agent sees which engines contributed, which were empty, which were filtered, and which errored - the fanout never silently hides a missing provider. Compact results: url, source (hostname), title, snippet, engine. Optional `domains` strictly filters returned hosts and also adds site: hints upstream; rejected results are reported, never silently leaked."
)]
pub async fn search(
    query: String,
    limit: Option<usize>,
    page: Option<usize>,
    domains: Option<Vec<String>>,
) -> Result<SearchOutcome, ToolError> {
    let relevance_query = query.trim().to_string();
    if relevance_query.is_empty() {
        return Err(ArgosError::InvalidQuery("query must not be empty".into()).into());
    }

    let domains = quality::normalize_domains(domains.as_deref())?;
    let provider_query = quality::scoped_query(&relevance_query, &domains);

    let config = Config::from_env();
    let router = MetasearchRouter::from_config(&config);
    let outcome = router
        .run(
            &provider_query,
            &relevance_query,
            clamp_limit(limit),
            clamp_page(page),
            &domains,
        )
        .await?;
    Ok(outcome)
}
