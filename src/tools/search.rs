use flojo_mcp::prelude::*;

use crate::config::Config;
use crate::error::ArgosError;
use crate::limits::{clamp_limit, clamp_page};
use crate::metasearch::MetasearchRouter;
use crate::quality;
use crate::types::SearchOutcome;

#[tool(
    description = "Web and academic search through a native metasearch router. `profile` picks the provider family: 'general' (default) uses keyless web engines (DuckDuckGo, Bing, Brave); 'academic' uses keyless public APIs (OpenAlex, Crossref, arXiv). The router runs a bounded first wave, fuses rankings with weighted RRF and canonical URL dedup, and immediately falls back to the next eligible provider when one rate-limits. Identical queries reuse a short local cache. Returns {results, providers (per-provider status), warnings} so the agent sees which engines contributed, which were empty, which were filtered, and which errored - the fanout never silently hides a missing provider. Compact results: url, source (hostname), title, snippet, engine. Optional `domains` strictly filters returned hosts and also adds site: hints upstream; rejected results are reported, never silently leaked.",
    read_only_hint = true,
    destructive_hint = false,
    idempotent_hint = true,
    open_world_hint = true
)]
pub async fn search(
    query: String,
    limit: Option<usize>,
    page: Option<usize>,
    domains: Option<Vec<String>>,
    profile: Option<String>,
) -> Result<SearchOutcome, ToolError> {
    let relevance_query = query.trim().to_string();
    if relevance_query.is_empty() {
        return Err(ArgosError::InvalidQuery("query must not be empty".into()).into());
    }

    let profile = profile
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(crate::metasearch::DEFAULT_PROFILE)
        .to_ascii_lowercase();

    let domains = quality::normalize_domains(domains.as_deref())?;
    let provider_query = quality::scoped_query(&relevance_query, &domains);

    let config = Config::from_env();
    // Profile is validated before any provider is contacted: a typo must never
    // cost provider quota.
    let router = MetasearchRouter::for_profile(&config, &profile)?;
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
