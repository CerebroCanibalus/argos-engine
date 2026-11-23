//! DBpedia adapter (`knowledge` profile).
//!
//! Public SPARQL endpoint, no key. Runs a full-text search over resource
//! labels and returns the matching entity URI with its comment.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "dbpedia";
const ENDPOINT: &str = "https://dbpedia.org/sparql";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    results: RawResults,
}

#[derive(Deserialize, Debug, Default)]
struct RawResults {
    #[serde(default)]
    bindings: Vec<RawBinding>,
}

#[derive(Deserialize, Debug, Default)]
struct RawBinding {
    #[serde(default)]
    s: Option<RawValue>,
    #[serde(default)]
    label: Option<RawValue>,
    #[serde(default)]
    comment: Option<RawValue>,
}

#[derive(Deserialize, Debug, Default)]
struct RawValue {
    #[serde(default)]
    value: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .results
        .bindings
        .into_iter()
        .filter_map(|b| {
            let uri = b.s.as_ref().and_then(|v| v.value.as_deref()).map(str::trim).filter(|u| !u.is_empty())?;
            let title = b
                .label
                .as_ref()
                .and_then(|v| v.value.as_deref())
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .unwrap_or(uri);
            let mut snippet = String::new();
            if let Some(comment) = b.comment.as_ref().and_then(|v| v.value.as_deref()).map(str::trim).filter(|c| !c.is_empty()) {
                snippet.push_str(comment);
                snippet.push_str(". ");
            }
            Some(build_result(ENGINE, uri, title, &snippet))
        })
        .collect())
}

pub struct DbpediaProvider {
    config: Config,
}

impl DbpediaProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for DbpediaProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 50);
        let sparql = format!(
            "SELECT ?s ?label ?comment WHERE {{ ?s rdfs:label ?label . OPTIONAL {{ ?s rdfs:comment ?comment }} FILTER (lang(?label) = 'en' && contains(lcase(str(?label)), lcase('{}'))) }} LIMIT {} OFFSET {}",
            query.replace('\'', "\\'"),
            per_page,
            (page.saturating_sub(1)) * per_page
        );
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("query", sparql),
                ("format", "application/sparql-results+json".to_string()),
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
        let sparql = "SELECT ?s WHERE { ?s rdfs:label ?l } LIMIT 1".to_string();
        match api_client()
            .get(ENDPOINT)
            .query(&[("query", sparql), ("format", "application/sparql-results+json".to_string())])
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
      "results": {
        "bindings": [
          {
            "s": { "value": "http://dbpedia.org/resource/Rust_(programming_language)" },
            "label": { "value": "Rust (programming language)" },
            "comment": { "value": "A systems programming language" }
          },
          { "s": { "value": "" } }
        ]
      }
    }"#;

    #[test]
    fn parses_bindings_and_skips_empty() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust (programming language)");
        assert!(results[0].snippet.contains("A systems programming language."));
        assert_eq!(results[0].url, "http://dbpedia.org/resource/Rust_(programming_language)");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
