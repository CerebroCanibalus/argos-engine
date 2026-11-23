//! Open-Meteo adapter (`knowledge` profile).
//!
//! Public weather forecast API, no key. Returns current conditions for a
//! given latitude/longitude (the query is parsed as "lat lon").

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "openmeteo";
const ENDPOINT: &str = "https://api.open-meteo.com/v1/forecast";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    latitude: Option<f64>,
    #[serde(default)]
    longitude: Option<f64>,
    #[serde(default)]
    current: Option<RawCurrent>,
}

#[derive(Deserialize, Debug, Default)]
struct RawCurrent {
    #[serde(default)]
    temperature_2m: Option<f64>,
    #[serde(default)]
    relative_humidity_2m: Option<i64>,
    #[serde(default)]
    wind_speed_10m: Option<f64>,
    #[serde(default)]
    weather_code: Option<i64>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    let lat = parsed.latitude.unwrap_or(0.0);
    let lon = parsed.longitude.unwrap_or(0.0);
    let current = parsed.current.unwrap_or_default();
    let mut snippet = String::new();
    if let Some(t) = current.temperature_2m {
        snippet.push_str(&format!("Temperature: {t} C. "));
    }
    if let Some(h) = current.relative_humidity_2m {
        snippet.push_str(&format!("Humidity: {h}%. "));
    }
    if let Some(w) = current.wind_speed_10m {
        snippet.push_str(&format!("Wind: {w} km/h. "));
    }
    if let Some(code) = current.weather_code {
        snippet.push_str(&format!("Weather code: {code}. "));
    }
    let url = "https://open-meteo.com/en/docs".to_string();
    let title = format!("Weather at {lat}, {lon}");
    Ok(vec![build_result(ENGINE, &url, &title, &snippet)])
}

pub struct OpenMeteoProvider {
    config: Config,
}

impl OpenMeteoProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for OpenMeteoProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        _page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let parts: Vec<&str> = query.split_whitespace().collect();
        let lat = parts.first().copied().unwrap_or("0");
        let lon = parts.get(1).copied().unwrap_or("0");
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("latitude", lat.to_string()),
                ("longitude", lon.to_string()),
                ("current", "temperature_2m,relative_humidity_2m,wind_speed_10m,weather_code".to_string()),
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
                ("latitude", "52.52".to_string()),
                ("longitude", "13.41".to_string()),
                ("current", "temperature_2m".to_string()),
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
      "latitude": 52.52,
      "longitude": 13.41,
      "current": {
        "temperature_2m": 12.3,
        "relative_humidity_2m": 60,
        "wind_speed_10m": 15.0,
        "weather_code": 2
      }
    }"#;

    #[test]
    fn parses_current_weather() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Weather at 52.52, 13.41");
        assert!(results[0].snippet.contains("Temperature: 12.3 C."));
        assert!(results[0].snippet.contains("Humidity: 60%."));
        assert!(results[0].snippet.contains("Wind: 15 km/h."));
        assert!(results[0].snippet.contains("Weather code: 2."));
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
