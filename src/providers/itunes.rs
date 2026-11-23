//! iTunes Search adapter (`knowledge` profile).
//!
//! Public search API, no key. Returns apps, music, books, podcasts and movies
//! matching the query with a canonical iTunes URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "itunes";
const ENDPOINT: &str = "https://itunes.apple.com/search";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default, rename = "results")]
    results: Vec<RawItem>,
}

#[derive(Deserialize, Debug, Default)]
struct RawItem {
    #[serde(default, rename = "trackName")]
    track_name: Option<String>,
    #[serde(default, rename = "collectionName")]
    collection_name: Option<String>,
    #[serde(default, rename = "artistName")]
    artist_name: Option<String>,
    #[serde(default, rename = "trackViewUrl")]
    track_url: Option<String>,
    #[serde(default, rename = "collectionViewUrl")]
    collection_url: Option<String>,
    #[serde(default, rename = "primaryGenreName")]
    genre: Option<String>,
    #[serde(default, rename = "releaseDate")]
    release_date: Option<String>,
    #[serde(default, rename = "kind")]
    kind: Option<String>,
    #[serde(default, rename = "wrapperType")]
    wrapper_type: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .results
        .into_iter()
        .filter_map(|item| {
            let title = item
                .track_name
                .as_deref()
                .or(item.collection_name.as_deref())
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = item
                .track_url
                .as_deref()
                .or(item.collection_url.as_deref())
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| "https://itunes.apple.com".to_string());
            let mut snippet = String::new();
            if let Some(artist) = item.artist_name.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
                snippet.push_str(&format!("{artist}. "));
            }
            if let Some(kind) = item.kind.as_deref().or(item.wrapper_type.as_deref()).map(str::trim).filter(|k| !k.is_empty()) {
                snippet.push_str(&format!("Type: {kind}. "));
            }
            if let Some(genre) = item.genre.as_deref().map(str::trim).filter(|g| !g.is_empty()) {
                snippet.push_str(&format!("Genre: {genre}. "));
            }
            if let Some(date) = item.release_date.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
                snippet.push_str(&format!("{date}. "));
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct ItunesProvider {
    config: Config,
}

impl ItunesProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for ItunesProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 200);
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("term", query.to_string()),
                ("limit", per_page.to_string()),
                ("media", "all".to_string()),
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
            .query(&[("term", "rust".to_string()), ("limit", "1".to_string())])
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
      "resultCount": 1,
      "results": [
        {
          "trackName": "Rust in Action",
          "artistName": "Tim McNamara",
          "trackViewUrl": "https://books.apple.com/us/book/rust-in-action/id123",
          "primaryGenreName": "Computers",
          "releaseDate": "2021-09-01",
          "kind": "book"
        },
        { "trackName": "" }
      ]
    }"#;

    #[test]
    fn parses_results_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust in Action");
        assert!(results[0].snippet.contains("Tim McNamara."));
        assert!(results[0].snippet.contains("Type: book."));
        assert!(results[0].snippet.contains("Genre: Computers."));
        assert!(results[0].snippet.contains("2021-09-01."));
        assert_eq!(results[0].url, "https://books.apple.com/us/book/rust-in-action/id123");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
