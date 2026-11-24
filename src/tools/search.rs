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
    freshness: Option<String>,
    strict: Option<bool>,
) -> Result<SearchOutcome, ToolError> {
    let relevance_query = query.trim().to_string();
    // Query rewriting: normalize + synonym expansion for upstream providers.
    let rewritten_query = crate::query_rewrite::rewrite(&relevance_query);
    if relevance_query.is_empty() {
        return Err(ArgosError::InvalidQuery("query must not be empty".into()).into());
    }

    let profile = profile
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(crate::metasearch::DEFAULT_PROFILE)
        .to_ascii_lowercase();

    let freshness = match freshness.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        None => None,
        Some(value) => {
            let value = value.to_ascii_lowercase();
            match value.as_str() {
                "day" | "week" | "month" | "year" | "all" => Some(value),
                _ => {
                    return Err(ArgosError::InvalidQuery(format!(
                        "freshness must be one of day, week, month, year, all (got {value:?})"
                    ))
                    .into())
                }
            }
        }
    };

    let strict = strict.unwrap_or(true);

    let domains = quality::normalize_domains(domains.as_deref())?;
    let provider_query = quality::scoped_query(&rewritten_query, &domains);

    let config = Config::from_env();
    // Profile is validated before any provider is contacted: a typo must never
    // cost provider quota.
    let router = MetasearchRouter::for_profile(&config, &profile)?;
    let mut outcome = router
        .run(
            &provider_query,
            &relevance_query,
            clamp_limit(limit),
            clamp_page(page),
            &domains,
            strict,
        )
        .await?;

    // Freshness filter (heuristic: uses published_at or a 4-digit year in the
    // snippet/title/url). Results without any date pass through.
    if let Some(freshness) = freshness.as_deref()
        && freshness != "all"
    {
            let current_year: i32 = 2026;
            let min_year = if freshness == "year" { current_year - 1 } else { current_year };
            outcome.results.retain(|r| {
                let year = r
                    .published_at
                    .as_deref()
                    .and_then(extract_year)
                    .or_else(|| extract_year(&format!("{} {} {}", r.title, r.snippet, r.url)));
                year.is_none_or(|y| y >= min_year)
            });
    }

    Ok(outcome)
}


#[allow(clippy::collapsible_if)]
fn extract_year(text: &str) -> Option<i32> {
    // Find the first 4-digit year in the 1900..=2100 range.
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
        {
            // Ensure word boundary-ish: not part of a longer digit run.
            let before_ok = i == 0 || !bytes[i - 1].is_ascii_digit();
            let after_ok = i + 4 == bytes.len() || !bytes[i + 4].is_ascii_digit();
            if before_ok && after_ok {
                if let Ok(s) = std::str::from_utf8(&bytes[i..i + 4]) {
                    if let Ok(y) = s.parse::<i32>() {
                        if (1900..=2100).contains(&y) {
                            return Some(y);
                        }
                    }
                }
            }
        }
        i += 1;
    }
    None
}
