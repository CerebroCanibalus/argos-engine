//! Nominatim (OpenStreetMap) adapter (`knowledge` profile).
//!
//! Public geocoding search, no key (usage policy: identify the client, which
//! the shared api_client does). Returns place name, type and a canonical
//! openstreetmap.org URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "nominatim";
const ENDPOINT: &str = "https://nominatim.openstreetmap.org/search";

#[derive(Deserialize, Debug, Default)]
struct RawPlace {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    #[serde(rename = "type")]
    type_: Option<String>,
    #[serde(default, rename = "class")]
    class: Option<String>,
    #[serde(default)]
    lat: Option<String>,
    #[serde(default)]
    lon: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: Vec<RawPlace> = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .into_iter()
        .filter_map(|place| {
            let title = place
                .display_name
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = match (place.lat.as_deref(), place.lon.as_deref()) {
                (Some(lat), Some(lon)) => format!(
                    "https://www.openstreetmap.org/?mlat={lat}&mlon={lon}#map=12/{lat}/{lon}"
                ),
                _ => "https://www.openstreetmap.org".to_string(),
            };
            let mut snippet = String::new();
            if let Some(class) = place
                .class
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
            {
                snippet.push_str(class);
                snippet.push_str(". ");
            }
            if let Some(t) = place
                .type_
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                snippet.push_str(&format!("Type: {t}. "));
            }
            if let (Some(lat), Some(lon)) = (place.lat.as_deref(), place.lon.as_deref()) {
                snippet.push_str(&format!("Coords: {lat}, {lon}. "));
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct NominatimProvider {
    config: Config,
}

impl NominatimProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for NominatimProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 50);
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("q", query.to_string()),
                ("format", "json".to_string()),
                ("limit", per_page.to_string()),
                ("addressdetails", "1".to_string()),
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
            .query(&[
                ("q", "Berlin".to_string()),
                ("format", "json".to_string()),
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

    const FIXTURE: &str = r#"[
      {
        "place_id": 123,
        "display_name": "Berlin, Germany",
        "class": "place",
        "type": "city",
        "lat": "52.52",
        "lon": "13.405"
      },
      { "display_name": "" }
    ]"#;

    #[test]
    fn parses_places_and_skips_nameless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Berlin, Germany");
        assert!(results[0].snippet.contains("place."));
        assert!(results[0].snippet.contains("Type: city."));
        assert!(results[0].snippet.contains("Coords: 52.52, 13.405."));
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
