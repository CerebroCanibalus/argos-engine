//! Open Library adapter (`knowledge` profile).
//!
//! Public search API for books. No key. Returns title, author, year and a
//! stable work URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "openlibrary";
const ENDPOINT: &str = "https://openlibrary.org/search.json";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    docs: Vec<RawDoc>,
}

#[derive(Deserialize, Debug, Default)]
struct RawDoc {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    author_name: Option<Vec<String>>,
    #[serde(default)]
    first_publish_year: Option<i64>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    subject: Option<Vec<String>>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .docs
        .into_iter()
        .filter_map(|doc| {
            let title = doc
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = doc
                .key
                .as_deref()
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .map(|k| format!("https://openlibrary.org{k}"))
                .unwrap_or_else(|| "https://openlibrary.org".to_string());
            let mut snippet = String::new();
            if let Some(authors) = doc.author_name.as_ref().filter(|a| !a.is_empty()) {
                snippet.push_str(&authors.join(", "));
                snippet.push_str(". ");
            }
            if let Some(year) = doc.first_publish_year {
                snippet.push_str(&format!("First published {year}. "));
            }
            if let Some(subjects) = doc
                .subject
                .as_ref()
                .map(|s| {
                    s.iter()
                        .map(|x| x.trim())
                        .filter(|x| !x.is_empty())
                        .take(4)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|s| !s.is_empty())
            {
                snippet.push_str(&format!("Subjects: {subjects}. "));
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct OpenLibraryProvider {
    config: Config,
}

impl OpenLibraryProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for OpenLibraryProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 100);
        let offset = (page.saturating_sub(1)) * per_page;
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("q", query.to_string()),
                ("limit", per_page.to_string()),
                ("offset", offset.to_string()),
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
            .query(&[("q", "rust".to_string()), ("limit", "1".to_string())])
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
      "docs": [
        {
          "title": "The Rust Programming Language",
          "author_name": ["Steve Klabnik", "Carol Nichols"],
          "first_publish_year": 2018,
          "key": "/works/OL17930368W",
          "subject": ["Rust (programming language)", "Programming"]
        },
        { "title": "", "key": "/works/OL1W" }
      ]
    }"#;

    #[test]
    fn parses_books_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "The Rust Programming Language");
        assert!(results[0].snippet.contains("Steve Klabnik, Carol Nichols."));
        assert!(results[0].snippet.contains("First published 2018."));
        assert!(
            results[0]
                .snippet
                .contains("Subjects: Rust (programming language), Programming.")
        );
        assert_eq!(results[0].url, "https://openlibrary.org/works/OL17930368W");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
