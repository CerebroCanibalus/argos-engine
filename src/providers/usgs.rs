//! USGS Earthquakes adapter (`knowledge` profile).
//!
//! Public earthquake catalog API, no key. Returns recent earthquakes with
//! magnitude, place, time and a canonical USGS URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "usgs";
const ENDPOINT: &str = "https://earthquake.usgs.gov/fdsnws/event/1/query";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    features: Vec<RawFeature>,
}

#[derive(Deserialize, Debug, Default)]
struct RawFeature {
    #[serde(default)]
    properties: Option<RawProps>,
}

#[derive(Deserialize, Debug, Default)]
struct RawProps {
    #[serde(default)]
    mag: Option<f64>,
    #[serde(default)]
    place: Option<String>,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .features
        .into_iter()
        .filter_map(|f| {
            let props = f.properties.unwrap_or_default();
            let title = props
                .title
                .as_deref()
                .or(props.place.as_deref())
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = props
                .url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| "https://earthquake.usgs.gov".to_string());
            let mut snippet = String::new();
            if let Some(mag) = props.mag {
                snippet.push_str(&format!("Magnitude: {mag}. "));
            }
            if let Some(place) = props.place.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
                snippet.push_str(&format!("{place}. "));
            }
            if let Some(time) = props.time {
                snippet.push_str(&format!("Time: {time}. "));
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct UsgsProvider {
    config: Config,
}

impl UsgsProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for UsgsProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        _page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 100);
        // USGS has no free-text query; use minmagnitude filter from query if numeric, else default.
        let min_mag = query.parse::<f64>().ok();
        let mut q = vec![
            ("format", "geojson".to_string()),
            ("limit", per_page.to_string()),
            ("orderby", "time".to_string()),
        ];
        if let Some(m) = min_mag {
            q.push(("minmagnitude", m.to_string()));
        }
        let response = api_client()
            .get(ENDPOINT)
            .query(&q)
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
        if _page > 1 {
            let skip = ((_page.saturating_sub(1)) * per_page).min(results.len());
            results.drain(..skip);
        }
        results.truncate(limit);
        Ok(results)
    }

    async fn health(&self) -> bool {
        match api_client()
            .get(ENDPOINT)
            .query(&[("format", "geojson".to_string()), ("limit", "1".to_string())])
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
      "features": [
        {
          "id": "abc",
          "properties": {
            "mag": 5.5,
            "place": "10 km N of Foo",
            "time": 1700000000000,
            "url": "https://earthquake.usgs.gov/earthquakes/eventpage/abc",
            "title": "M 5.5 - 10 km N of Foo"
          }
        },
        { "properties": { "title": "" } }
      ]
    }"#;

    #[test]
    fn parses_quakes_and_skips_titleless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "M 5.5 - 10 km N of Foo");
        assert!(results[0].snippet.contains("Magnitude: 5.5."));
        assert!(results[0].snippet.contains("10 km N of Foo."));
        assert!(results[0].snippet.contains("Time: 1700000000000."));
        assert_eq!(results[0].url, "https://earthquake.usgs.gov/earthquakes/eventpage/abc");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
