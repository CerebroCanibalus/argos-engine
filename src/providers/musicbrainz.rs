//! MusicBrainz adapter (`knowledge` profile).
//!
//! Public search API, no key (requires a descriptive User-Agent, which the
//! shared api_client sends). Returns artists/recordings/releases.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "musicbrainz";
const ENDPOINT: &str = "https://musicbrainz.org/ws/2";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    artists: Option<Vec<RawArtist>>,
    #[serde(default)]
    recordings: Option<Vec<RawRecording>>,
    #[serde(default)]
    releases: Option<Vec<RawRelease>>,
}

#[derive(Deserialize, Debug, Default)]
struct RawArtist {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "type")]
    type_: Option<String>,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    disambiguation: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct RawRecording {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    length: Option<i64>,
}

#[derive(Deserialize, Debug, Default)]
struct RawRelease {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    date: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    // Prefer artists if present, else recordings, else releases.
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    let mut results = Vec::new();
    if let Some(artists) = parsed.artists.filter(|a| !a.is_empty()) {
        for a in artists {
            if let Some(name) = a.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                let id = a.id.as_deref().unwrap_or("");
                let url = if id.is_empty() {
                    "https://musicbrainz.org".to_string()
                } else {
                    format!("https://musicbrainz.org/artist/{id}")
                };
                let mut snippet = String::new();
                if let Some(t) = a.type_.as_deref() {
                    snippet.push_str(&format!("{t}. "));
                }
                if let Some(c) = a.country.as_deref() {
                    snippet.push_str(&format!("Country: {c}. "));
                }
                if let Some(d) = a
                    .disambiguation
                    .as_deref()
                    .map(str::trim)
                    .filter(|d| !d.is_empty())
                {
                    snippet.push_str(d);
                    snippet.push_str(". ");
                }
                results.push(build_result(ENGINE, &url, name, &snippet));
            }
        }
    } else if let Some(recordings) = parsed.recordings.filter(|r| !r.is_empty()) {
        for r in recordings {
            if let Some(title) = r.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                let id = r.id.as_deref().unwrap_or("");
                let url = if id.is_empty() {
                    "https://musicbrainz.org".to_string()
                } else {
                    format!("https://musicbrainz.org/recording/{id}")
                };
                let mut snippet = String::new();
                if let Some(l) = r.length {
                    snippet.push_str(&format!("Length: {}ms. ", l));
                }
                results.push(build_result(ENGINE, &url, title, &snippet));
            }
        }
    } else if let Some(releases) = parsed.releases.filter(|r| !r.is_empty()) {
        for r in releases {
            if let Some(title) = r.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                let id = r.id.as_deref().unwrap_or("");
                let url = if id.is_empty() {
                    "https://musicbrainz.org".to_string()
                } else {
                    format!("https://musicbrainz.org/release/{id}")
                };
                let mut snippet = String::new();
                if let Some(d) = r.date.as_deref() {
                    snippet.push_str(&format!("{d}. "));
                }
                results.push(build_result(ENGINE, &url, title, &snippet));
            }
        }
    }
    Ok(results)
}

pub struct MusicBrainzProvider {
    config: Config,
}

impl MusicBrainzProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for MusicBrainzProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 100);
        let response = api_client()
            .get(format!("{ENDPOINT}/artist"))
            .query(&[
                ("query", query.to_string()),
                ("fmt", "json".to_string()),
                ("limit", per_page.to_string()),
                ("offset", ((page.saturating_sub(1)) * per_page).to_string()),
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
            .get(format!("{ENDPOINT}/artist"))
            .query(&[
                ("query", "beatles".to_string()),
                ("fmt", "json".to_string()),
                ("limit", "1".to_string()),
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
      "artists": [
        {
          "id": "b10bbbfc-cf9e-42e0-be17-e2c3e1d2600d",
          "name": "The Beatles",
          "type": "Group",
          "country": "GB",
          "disambiguation": "English rock band"
        },
        { "name": "" }
      ]
    }"#;

    #[test]
    fn parses_artists_and_skips_nameless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "The Beatles");
        assert!(results[0].snippet.contains("Group."));
        assert!(results[0].snippet.contains("Country: GB."));
        assert!(results[0].snippet.contains("English rock band."));
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
