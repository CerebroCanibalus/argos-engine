//! Homebrew adapter (`code` profile).
//!
//! Public formulae JSON endpoint, no key. The endpoint returns the whole
//! catalogue, so the adapter filters client-side by name/description.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "homebrew";
const ENDPOINT: &str = "https://formulae.brew.sh/api/formula.json";

#[derive(Deserialize, Debug, Default)]
struct RawFormula {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    desc: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    versions: Option<RawVersions>,
    #[serde(default)]
    license: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct RawVersions {
    #[serde(default)]
    stable: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: Vec<RawFormula> = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .into_iter()
        .filter_map(|f| {
            let name = f.name.as_deref().map(str::trim).filter(|n| !n.is_empty())?;
            let url = f
                .homepage
                .as_deref()
                .map(str::trim)
                .filter(|h| !h.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("https://formulae.brew.sh/formula/{name}"));
            let mut snippet = String::new();
            if let Some(desc) = f.desc.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
                snippet.push_str(desc);
                snippet.push_str(". ");
            }
            if let Some(version) = f
                .versions
                .as_ref()
                .and_then(|v| v.stable.as_deref())
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                snippet.push_str(&format!("v{version}. "));
            }
            if let Some(license) = f
                .license
                .as_deref()
                .map(str::trim)
                .filter(|l| !l.is_empty())
            {
                snippet.push_str(&format!("License: {license}. "));
            }
            Some(build_result(ENGINE, &url, name, &snippet))
        })
        .collect())
}

pub struct HomebrewProvider {
    config: Config,
}

impl HomebrewProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for HomebrewProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 100);
        let needle = query.to_lowercase();
        let response = api_client()
            .get(ENDPOINT)
            .timeout(self.config.request_timeout)
            .send()
            .await
            .map_err(|ex| ArgosError::Unreachable {
                origin: ENGINE.into(),
                cause: ex.to_string(),
            })?;

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            return Err(map_api_status(ENGINE, code, retry_after_of(&response)));
        }
        let payload = response
            .text()
            .await
            .map_err(|ex| ArgosError::Unreachable {
                origin: ENGINE.into(),
                cause: ex.to_string(),
            })?;
        let mut all = parse_results(&payload)?;
        // Client-side filter because the endpoint has no search parameter.
        all.retain(|r| {
            r.title.to_lowercase().contains(&needle) || r.snippet.to_lowercase().contains(&needle)
        });
        if page > 1 {
            let skip = ((page.saturating_sub(1)) * per_page).min(all.len());
            all.drain(..skip);
        }
        all.truncate(limit);
        Ok(all)
    }

    async fn health(&self) -> bool {
        match api_client()
            .get(ENDPOINT)
            .timeout(self.config.ping_timeout)
            .send()
            .await
        {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"[
      {
        "name": "rust",
        "desc": "Safe, concurrent, practical language",
        "homepage": "https://www.rust-lang.org/",
        "versions": { "stable": "1.75.0" },
        "license": "Apache-2.0"
      },
      { "name": "" }
    ]"#;

    #[test]
    fn parses_formulae_and_skips_nameless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "rust");
        assert!(
            results[0]
                .snippet
                .contains("Safe, concurrent, practical language.")
        );
        assert!(results[0].snippet.contains("v1.75.0."));
        assert!(results[0].snippet.contains("License: Apache-2.0."));
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
