//! REST Countries adapter (`knowledge` profile).
//!
//! Public country data API, no key. Searches by name and returns capital,
//! region, population and a canonical flag URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "restcountries";
const ENDPOINT: &str = "https://restcountries.com/v3.1/name";

#[derive(Deserialize, Debug, Default)]
struct RawCountry {
    #[serde(default)]
    name: Option<RawName>,
    #[serde(default)]
    capital: Option<Vec<String>>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    subregion: Option<String>,
    #[serde(default)]
    population: Option<i64>,
    #[serde(default)]
    flags: Option<RawFlags>,
}

#[derive(Deserialize, Debug, Default)]
struct RawName {
    #[serde(default)]
    common: Option<String>,
    #[serde(default)]
    official: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct RawFlags {
    #[serde(default)]
    png: Option<String>,
    #[serde(default)]
    svg: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: Vec<RawCountry> = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .into_iter()
        .filter_map(|country| {
            let name = country
                .name
                .as_ref()
                .and_then(|n| n.common.as_deref().or(n.official.as_deref()))
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let url = country
                .flags
                .as_ref()
                .and_then(|f| f.png.as_deref().or(f.svg.as_deref()))
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| "https://restcountries.com".to_string());
            let mut snippet = String::new();
            if let Some(capital) = country.capital.as_ref().filter(|c| !c.is_empty()) {
                snippet.push_str(&format!("Capital: {}. ", capital.join(", ")));
            }
            if let Some(region) = country
                .region
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
            {
                snippet.push_str(&format!("{region}. "));
            }
            if let Some(subregion) = country
                .subregion
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                snippet.push_str(&format!("{subregion}. "));
            }
            if let Some(population) = country.population {
                snippet.push_str(&format!("Population: {population}. "));
            }
            Some(build_result(ENGINE, &url, name, &snippet))
        })
        .collect())
}

pub struct RestCountriesProvider {
    config: Config,
}

impl RestCountriesProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for RestCountriesProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 50);
        let response = api_client()
            .get(format!("{ENDPOINT}/{query}"))
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
            .get(format!("{ENDPOINT}/germany"))
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
        "name": { "common": "Germany", "official": "Federal Republic of Germany" },
        "capital": ["Berlin"],
        "region": "Europe",
        "subregion": "Western Europe",
        "population": 83240525,
        "flags": { "png": "https://flagcdn.com/w320/de.png" },
        "languages": { "deu": "German" }
      },
      { "name": { "common": "" } }
    ]"#;

    #[test]
    fn parses_countries_and_skips_nameless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Germany");
        assert!(results[0].snippet.contains("Capital: Berlin."));
        assert!(results[0].snippet.contains("Europe."));
        assert!(results[0].snippet.contains("Western Europe."));
        assert!(results[0].snippet.contains("Population: 83240525."));
        assert_eq!(results[0].url, "https://flagcdn.com/w320/de.png");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
