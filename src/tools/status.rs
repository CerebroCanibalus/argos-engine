use flojo_mcp::prelude::*;

use crate::config::Config;
use crate::metasearch::{DEFAULT_PROFILE, MetasearchRouter};
use crate::types::Status;

#[tool(
    description = "Engine status: version, the configured SearXNG endpoint, and reachability probes for the providers of one profile. `profile` accepts 'general' (default), 'academic', 'code', 'news' or 'knowledge'; only that profile's providers are probed, so status stays cheap and does not spend quota on families the caller is not searching.",
    read_only_hint = true,
    destructive_hint = false,
    idempotent_hint = true,
    open_world_hint = true
)]
pub async fn status(profile: Option<String>) -> Result<Status, ToolError> {
    let config = Config::from_env();
    let profile = profile
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_PROFILE)
        .to_ascii_lowercase();

    // The router validates the profile and exposes exactly the members it would
    // search, so the probes match what a search would actually use.
    let router = MetasearchRouter::for_profile(&config, &profile)?;
    let searxng_reachable = router.searxng_reachable().await;
    Ok(Status {
        name: "argos-engine".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        profile,
        searxng_url: config.searxng_url,
        searxng_reachable,
        providers: router.probes().await,
    })
}
