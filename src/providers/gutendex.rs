//! Gutendex adapter (Project Gutenberg) (`knowledge` profile).
//!
//! Public books API, no key. Returns title, authors and a canonical
//! gutenberg.org URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "gutendex";
const ENDPOINT: &str = "https://gutendex.com/books";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    results: Vec<RawBook>,
}

#[derive(Deserialize, Debug, Default)]
struct RawBook {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    authors: Option<Vec<RawAuthor>>,
    #[serde(default)]
    subjects: Option<Vec<String>>,
    #[serde(default)]
    download_count: Option<i64>,
}

#[derive(Deserialize, Debug, Default)]
struct RawAuthor {
    #[serde(default)]
    name: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .results
        .into_iter()
        .filter_map(|book| {
            let title = book
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = book
                .id
                .map(|id| format!("https://www.gutenberg.org/ebooks/{id}"))
                .unwrap_or_else(|| "https://www.gutenberg.org".to_string());
            let mut snippet = String::new();
            if let Some(authors) = book.authors.as_ref().filter(|a| !a.is_empty()) {
                let names: Vec<&str> = authors
                    .iter()
                    .filter_map(|a| a.name.as_deref().map(str::trim).filter(|n| !n.is_empty()))
                    .collect();
                if !names.is_empty() {
                    snippet.push_str(&names.join(", "));
                    snippet.push_str(". ");
                }
            }
            if let Some(subjects) = book.subjects.as_ref().filter(|s| !s.is_empty()) {
                snippet.push_str(&format!(
                    "Subjects: {}. ",
                    subjects
                        .iter()
                        .take(4)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if let Some(downloads) = book.download_count {
                snippet.push_str(&format!("{downloads} downloads. "));
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct GutendexProvider {
    config: Config,
}

impl GutendexProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for GutendexProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 32);
        let response = api_client()
            .get(ENDPOINT)
            .query(&[("search", query.to_string())])
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
        if page > 1 {
            let skip = ((page.saturating_sub(1)) * per_page).min(results.len());
            results.drain(..skip);
        }
        results.truncate(limit);
        Ok(results)
    }

    async fn health(&self) -> bool {
        match api_client()
            .get(ENDPOINT)
            .query(&[("search", "rust".to_string())])
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
      "results": [
        {
          "id": 12345,
          "title": "Rust for Beginners",
          "authors": [{ "name": "Alice Smith" }],
          "subjects": ["Programming", "Rust"],
          "download_count": 500
        },
        { "id": 1, "title": "" }
      ]
    }"#;

    #[test]
    fn parses_books_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust for Beginners");
        assert!(results[0].snippet.contains("Alice Smith."));
        assert!(results[0].snippet.contains("Subjects: Programming, Rust."));
        assert!(results[0].snippet.contains("500 downloads."));
        assert_eq!(results[0].url, "https://www.gutenberg.org/ebooks/12345");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
