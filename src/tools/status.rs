use flojo_mcp::prelude::*;

use crate::config::Config;
use crate::providers::SearchProvider;
use crate::providers::searxng::SearxNgProvider;
use crate::types::Status;

#[tool(description = "Engine status: version, configuration and local SearXNG reachability")]
pub async fn status() -> Result<Status, ToolError> {
    let config = Config::from_env();
    let provider = SearxNgProvider::new(config.clone());
    let searxng_reachable = provider.health().await;
    Ok(Status {
        name: "argos-engine".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        searxng_url: config.searxng_url,
        searxng_reachable,
    })
}
