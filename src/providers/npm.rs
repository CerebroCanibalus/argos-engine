//! npm registry search adapter (`P1_VERTICAL`, code profile).
//!
//! npm's public search endpoint powers `npm search` in the CLI and needs no key.
//! A library request for the JavaScript ecosystem should hit the registry index
//! rather than a web engine that will rank blog posts above packages.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{
    SearchProvider, api_client, build_result, map_api_status, offset_of, retry_after_of,
};

const ENGINE: &str = "npm";
const ENDPOINT: &str = "https://registry.npmjs.org/-/v1/search";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    objects: Vec<RawObject>,
}

#[derive(Deserialize, Debug, Default)]
struct RawObject {
    #[serde(default)]
    package: RawPackage,
}

#[derive(Deserialize, Debug, Default)]
struct RawPackage {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    links: RawLinks,
    #[serde(default)]
    publisher: Option<RawAuthor>,
}

#[derive(Deserialize, Debug, Default)]
struct RawLinks {
    #[serde(default)]
    npm: String,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    repository: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct RawAuthor {
    #[serde(default)]
    username: String,
}

/// Landing page: the repository is more informative than the npm page, but the
/// npm page always exists and is the canonical distribution home.
fn canonical_url(package: &RawPackage) -> String {
    let name = package.name.trim();
    if let Some(repo) = package
        .links
        .repository
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        // npm returns git+ssh/git+https forms; normalise to a browsable URL.
        let cleaned = repo
            .trim_start_matches("git+")
            .trim_start_matches("git://")
            .trim_end_matches(".git");
        if cleaned.starts_with("http") {
            return cleaned.to_string();
        }
    }
    if let Some(homepage) = package
        .links
        .homepage
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty() && u.starts_with("http"))
    {
        return homepage.to_string();
    }
    let npm = package.links.npm.trim();
    if !npm.is_empty() {
        return npm.to_string();
    }
    format!("https://www.npmjs.com/package/{name}")
}

/// Parse an npm registry search payload into compact results (pure).
pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .objects
        .into_iter()
        .filter_map(|object| {
            let package = object.package;
            let name = package.name.trim();
            if name.is_empty() {
                return None;
            }
            let url = canonical_url(&package);
            let mut snippet = String::new();
            if let Some(description) = package
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(description);
                snippet.push_str(". ");
            }
            let version = package.version.trim();
            if !version.is_empty() {
                snippet.push_str(&format!("v{version}. "));
            }
            let maintainer = package
                .publisher
                .as_ref()
                .map(|p| p.username.trim())
                .filter(|p| !p.is_empty());
            if let Some(maintainer) = maintainer {
                snippet.push_str(&format!("by {maintainer}. "));
            }
            if !package.keywords.is_empty() {
                let keywords = package
                    .keywords
                    .iter()
                    .map(|k| k.trim())
                    .filter(|k| !k.is_empty())
                    .take(4)
                    .collect::<Vec<_>>()
                    .join(", ");
                if !keywords.is_empty() {
                    snippet.push_str(&format!("Keywords: {keywords}. "));
                }
            }
            if let Some(date) = package
                .date
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                // Only the date part; the full timestamp wastes the budget.
                snippet.push_str(&format!("Updated {}.", date.get(..10).unwrap_or(date)));
            }
            Some(build_result(ENGINE, &url, name, &snippet))
        })
        .collect())
}

/// npm registry provider over the shared public-API HTTP client.
pub struct NpmProvider {
    config: Config,
}

impl NpmProvider {
    /// Create the provider for the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for NpmProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let size = limit.clamp(1, 250);
        let offset = offset_of(page, size);
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("text", query.to_string()),
                ("size", size.to_string()),
                ("from", offset.to_string()),
            ])
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
        let mut results = parse_results(&payload)?;
        results.truncate(limit);
        Ok(results)
    }

    async fn health(&self) -> bool {
        match api_client()
            .get(ENDPOINT)
            .query(&[("text", "react"), ("size", "1")])
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

    const FIXTURE: &str = r#"{
      "total": 100,
      "objects": [
        {
          "package": {
            "name": "react-router",
            "version": "6.30.0",
            "description": "Declarative routing for React",
            "keywords": ["react", "router"],
            "date": "2025-01-15T10:20:30.000Z",
            "links": {
              "npm": "https://www.npmjs.com/package/react-router",
              "homepage": "https://reactrouter.com",
              "repository": "git+https://github.com/remix-run/react-router.git"
            },
            "publisher": {"username": "remix-run"}
          },
          "score": {"final": 0.9}
        },
        {
          "package": { "name": "no-links", "version": "1.0.0" }
        },
        { "package": { "name": "" } }
      ]
    }"#;

    #[test]
    fn normalises_git_urls_to_the_repository() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 2, "a package without a name is unusable");
        assert_eq!(results[0].url, "https://github.com/remix-run/react-router");
        assert_eq!(results[0].title, "react-router");
    }

    #[test]
    fn falls_back_to_the_npm_page_and_trims_timestamps() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results[1].url, "https://www.npmjs.com/package/no-links");
        let snippet = &results[0].snippet;
        assert!(snippet.starts_with("Declarative routing for React. v6.30.0."));
        assert!(snippet.contains("by remix-run."));
        assert!(snippet.contains("Updated 2025-01-15."));
        assert!(
            !snippet.contains("T10:20"),
            "timestamp wastes budget: {snippet}"
        );
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
        assert!(parse_results("{\"objects\": []}").is_ok_and(|r| r.is_empty()));
    }
}
