use flojo_mcp::prelude::*;

use crate::config::Config;
use crate::providers::SearchProvider;
use crate::providers::fanout::Fanout;
use crate::providers::searxng::SearxNgProvider;
use crate::types::Status;

#[tool(
    description = "Engine status: version, configured providers with reachability probes, and the optional SearXNG endpoint"
)]
pub async fn status() -> Result<Status, ToolError> {
    let config = Config::from_env();
    let fanout = Fanout::from_config(&config);
    let providers = fanout.probes().await;
    let searxng_reachable = SearxNgProvider::new(config.clone()).health().await;
    Ok(Status {
        name: "argos-engine".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        searxng_url: config.searxng_url,
        searxng_reachable,
        providers,
    })
}
