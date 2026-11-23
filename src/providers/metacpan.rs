//! MetaCPAN adapter (`code` profile).
//!
//! Public CPAN module search, no key. Returns module name, abstract and a
//! canonical metacpan.org URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "metacpan";
const ENDPOINT: &str = "https://fastapi.metacpan.org/v1/module/_search";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    hits: RawHits,
}

#[derive(Deserialize, Debug, Default)]
struct RawHits {
    #[serde(default)]
    hits: Vec<RawHit>,
}

#[derive(Deserialize, Debug, Default)]
struct RawHit {
    #[serde(default, rename = "_source")]
    source: RawSource,
}

#[derive(Deserialize, Debug, Default)]
struct RawSource {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    abstract_: Option<String>,
    #[serde(default, rename = "abstract")]
    abstract_alt: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    version: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .hits
        .hits
        .into_iter()
        .filter_map(|hit| {
            let src = hit.source;
            let name = src
                .name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())?;
            let url = format!("https://metacpan.org/pod/{name}");
            let mut snippet = String::new();
            let abstract_text = src
                .abstract_
                .as_deref()
                .or(src.abstract_alt.as_deref())
                .map(str::trim)
                .filter(|a| !a.is_empty());
            if let Some(a) = abstract_text {
                snippet.push_str(a);
                snippet.push_str(". ");
            }
            if let Some(author) = src
                .author
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())
            {
                snippet.push_str(&format!("By {author}. "));
            }
            if let Some(version) = src
                .version
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                snippet.push_str(&format!("v{version}. "));
            }
            Some(build_result(ENGINE, &url, name, &snippet))
        })
        .collect())
}

pub struct MetacpanProvider {
    config: Config,
}

impl MetacpanProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for MetacpanProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 100);
        let body = flojo_mcp::serde_json::json!({
            "query": { "match": { "documentation": query } },
            "size": per_page,
            "from": (page.saturating_sub(1)) * per_page,
        });
        let response = api_client()
            .post(ENDPOINT)
            .json(&body)
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
        let body = flojo_mcp::serde_json::json!({ "query": { "match_all": {} }, "size": 1 });
        match api_client()
            .post(ENDPOINT)
            .json(&body)
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
      "hits": {
        "hits": [
          {
            "_source": {
              "name": "Moose",
              "abstract": "A postmodern object system for Perl 5",
              "author": "ETHER",
              "version": "2.2201"
            }
          },
          { "_source": { "name": "" } }
        ]
      }
    }"#;

    #[test]
    fn parses_modules_and_skips_nameless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Moose");
        assert!(
            results[0]
                .snippet
                .contains("A postmodern object system for Perl 5.")
        );
        assert!(results[0].snippet.contains("By ETHER."));
        assert!(results[0].snippet.contains("v2.2201."));
        assert_eq!(results[0].url, "https://metacpan.org/pod/Moose");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
