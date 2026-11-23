//! WordPress.org plugins adapter (`code` profile).
//!
//! Public plugins info API, no key. Returns plugin name, description,
//! rating and a canonical wordpress.org URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "wordpress";
const ENDPOINT: &str = "https://api.wordpress.org/plugins/info/1.2/";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    plugins: Vec<RawPlugin>,
}

#[derive(Deserialize, Debug, Default)]
struct RawPlugin {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    short_description: Option<String>,
    #[serde(default)]
    rating: Option<f64>,
    #[serde(default)]
    downloaded: Option<i64>,
    #[serde(default)]
    version: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    // The real endpoint returns { "plugins": [...] } when queried with action=query_plugins.
    // Accept both a wrapped object and a bare array for test fixtures.
    let plugins: Vec<RawPlugin> =
        if let Ok(wrapped) = flojo_mcp::serde_json::from_str::<RawResponse>(payload) {
            wrapped.plugins
        } else if let Ok(arr) = flojo_mcp::serde_json::from_str::<Vec<RawPlugin>>(payload) {
            arr
        } else {
            return Err(ArgosError::Decode("unexpected wordpress payload".into()));
        };
    Ok(plugins
        .into_iter()
        .filter_map(|p| {
            let name = p.name.as_deref().map(str::trim).filter(|n| !n.is_empty())?;
            let slug = p
                .slug
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("");
            let url = if slug.is_empty() {
                "https://wordpress.org/plugins/".to_string()
            } else {
                format!("https://wordpress.org/plugins/{slug}/")
            };
            let mut snippet = String::new();
            if let Some(desc) = p
                .short_description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(desc);
                snippet.push_str(". ");
            }
            if let Some(version) = p
                .version
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                snippet.push_str(&format!("v{version}. "));
            }
            if let Some(rating) = p.rating {
                snippet.push_str(&format!("Rating: {rating:.1}. "));
            }
            if let Some(downloads) = p.downloaded {
                snippet.push_str(&format!("{downloads} downloads. "));
            }
            Some(build_result(ENGINE, &url, name, &snippet))
        })
        .collect())
}

pub struct WordPressProvider {
    config: Config,
}

impl WordPressProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for WordPressProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 100);
        // WordPress expects form-encoded-ish; use GET with query params instead.
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("action", "query_plugins".to_string()),
                ("request[search]", query.to_string()),
                ("request[per_page]", per_page.to_string()),
                ("request[page]", page.to_string()),
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
            .query(&[
                ("action", "query_plugins".to_string()),
                ("request[search]", "seo".to_string()),
                ("request[per_page]", "1".to_string()),
            ])
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
      "plugins": [
        {
          "name": "Yoast SEO",
          "slug": "wordpress-seo",
          "short_description": "Improve your WordPress SEO",
          "rating": 98.0,
          "downloaded": 50000000,
          "version": "21.0"
        },
        { "name": "" }
      ]
    }"#;

    #[test]
    fn parses_plugins_and_skips_nameless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Yoast SEO");
        assert!(results[0].snippet.contains("Improve your WordPress SEO."));
        assert!(results[0].snippet.contains("v21.0."));
        assert!(results[0].snippet.contains("Rating: 98.0."));
        assert!(results[0].snippet.contains("50000000 downloads."));
        assert_eq!(
            results[0].url,
            "https://wordpress.org/plugins/wordpress-seo/"
        );
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
