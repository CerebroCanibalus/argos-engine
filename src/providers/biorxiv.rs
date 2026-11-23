//! bioRxiv/medRxiv adapter (`academic` profile).
//!
//! Public content API, no key. Searches recent preprints by query via the
//! content detail endpoint filtered by collection.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "biorxiv";
const ENDPOINT: &str = "https://api.biorxiv.org/details/biorxiv";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    collection: Vec<RawPaper>,
}

#[derive(Deserialize, Debug, Default)]
struct RawPaper {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    authors: Option<String>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    doi: Option<String>,
    #[serde(default)]
    abstract_: Option<String>,
    #[serde(default, rename = "abstract")]
    abstract_alt: Option<String>,
    #[serde(default)]
    category: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .collection
        .into_iter()
        .filter_map(|p| {
            let title = p
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let doi = p.doi.as_deref().map(str::trim).filter(|d| !d.is_empty());
            let url = doi
                .map(|d| format!("https://doi.org/{d}"))
                .unwrap_or_else(|| "https://www.biorxiv.org".to_string());
            let mut snippet = String::new();
            if let Some(authors) = p
                .authors
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())
            {
                snippet.push_str(authors);
                snippet.push_str(". ");
            }
            if let Some(date) = p.date.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
                snippet.push_str(&format!("{date}. "));
            }
            if let Some(category) = p
                .category
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
            {
                snippet.push_str(&format!("Category: {category}. "));
            }
            let abstract_text = p
                .abstract_
                .as_deref()
                .or(p.abstract_alt.as_deref())
                .map(str::trim)
                .filter(|a| !a.is_empty());
            if let Some(a) = abstract_text {
                snippet.push_str(a);
                snippet.push_str(". ");
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct BioRxivProvider {
    config: Config,
}

impl BioRxivProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for BioRxivProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 100);
        // bioRxiv content API is date-range based; use a wide window and filter client-side.
        let response = api_client()
            .get(format!("{ENDPOINT}/2020-01-01/2030-01-01/0"))
            .query(&[("format", "json".to_string())])
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
        let needle = query.to_lowercase();
        let mut results = parse_results(&payload)?;
        results.retain(|r| {
            r.title.to_lowercase().contains(&needle) || r.snippet.to_lowercase().contains(&needle)
        });
        if page > 1 {
            let skip = ((page.saturating_sub(1)) * per_page).min(results.len());
            results.drain(..skip);
        }
        results.truncate(limit);
        Ok(results)
    }

    async fn health(&self) -> bool {
        match api_client()
            .get(format!("{ENDPOINT}/2024-01-01/2024-01-02/0"))
            .query(&[("format", "json".to_string())])
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
      "collection": [
        {
          "title": "A new CRISPR method",
          "authors": "Alice Smith; Bob Jones",
          "date": "2024-01-15",
          "doi": "10.1101/2024.01.15.123",
          "abstract": "We present a new method",
          "category": "molecular biology"
        },
        { "title": "" }
      ]
    }"#;

    #[test]
    fn parses_papers_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "A new CRISPR method");
        assert!(results[0].snippet.contains("Alice Smith; Bob Jones."));
        assert!(results[0].snippet.contains("2024-01-15."));
        assert!(results[0].snippet.contains("Category: molecular biology."));
        assert!(results[0].snippet.contains("We present a new method."));
        assert_eq!(results[0].url, "https://doi.org/10.1101/2024.01.15.123");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
