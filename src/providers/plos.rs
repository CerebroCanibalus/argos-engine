//! PLOS (Public Library of Science) adapter (`academic` profile).
//!
//! Public search API, no key. Returns articles with title, authors, journal,
//! publication date and a canonical plos.org URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "plos";
const ENDPOINT: &str = "https://api.plos.org/search";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    response: RawInner,
}

#[derive(Deserialize, Debug, Default)]
struct RawInner {
    #[serde(default)]
    docs: Vec<RawDoc>,
}

#[derive(Deserialize, Debug, Default)]
struct RawDoc {
    #[serde(default, rename = "id")]
    id: Option<String>,
    #[serde(default, rename = "title_display")]
    title: Option<String>,
    #[serde(default, rename = "author_display")]
    authors: Option<Vec<String>>,
    #[serde(default, rename = "journal")]
    journal: Option<String>,
    #[serde(default, rename = "publication_date")]
    publication_date: Option<String>,
    #[serde(default, rename = "abstract")]
    abstract_: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .response
        .docs
        .into_iter()
        .filter_map(|d| {
            let title = d
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let id =
                d.id.as_deref()
                    .map(str::trim)
                    .filter(|i| !i.is_empty())
                    .unwrap_or("");
            let url = if id.is_empty() {
                "https://journals.plos.org".to_string()
            } else {
                format!("https://journals.plos.org/plosone/article?id={id}")
            };
            let mut snippet = String::new();
            if let Some(authors) = d.authors.as_ref().filter(|a| !a.is_empty()) {
                snippet.push_str(&authors.join(", "));
                snippet.push_str(". ");
            }
            if let Some(journal) = d
                .journal
                .as_deref()
                .map(str::trim)
                .filter(|j| !j.is_empty())
            {
                snippet.push_str(&format!("{journal}. "));
            }
            if let Some(date) = d
                .publication_date
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                snippet.push_str(&format!("{date}. "));
            }
            if let Some(abs) = d
                .abstract_
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())
            {
                snippet.push_str(abs);
                snippet.push_str(". ");
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct PlosProvider {
    config: Config,
}

impl PlosProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for PlosProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 100);
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("q", query.to_string()),
                ("rows", per_page.to_string()),
                ("start", ((page.saturating_sub(1)) * per_page).to_string()),
                ("wt", "json".to_string()),
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
                ("q", "rust".to_string()),
                ("rows", "1".to_string()),
                ("wt", "json".to_string()),
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
      "response": {
        "docs": [
          {
            "id": "10.1371/journal.pone.0123",
            "title_display": "A Rust paper",
            "author_display": ["Alice Smith", "Bob Jones"],
            "journal": "PLOS ONE",
            "publication_date": "2024-01-01",
            "abstract": "We study Rust"
          },
          { "title_display": "" }
        ]
      }
    }"#;

    #[test]
    fn parses_articles_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "A Rust paper");
        assert!(results[0].snippet.contains("Alice Smith, Bob Jones."));
        assert!(results[0].snippet.contains("PLOS ONE."));
        assert!(results[0].snippet.contains("2024-01-01."));
        assert!(results[0].snippet.contains("We study Rust."));
        assert_eq!(
            results[0].url,
            "https://journals.plos.org/plosone/article?id=10.1371/journal.pone.0123"
        );
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
