//! Frankfurter adapter (`knowledge` profile).
//!
//! Public currency rates API, no key. Returns conversion rates for a base
//! currency against a set of targets on a given (or latest) date.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "frankfurter";
const ENDPOINT: &str = "https://api.frankfurter.app";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    rates: Option<std::collections::HashMap<String, f64>>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    let base = parsed.base.as_deref().map(str::trim).filter(|b| !b.is_empty()).unwrap_or("EUR");
    let date = parsed.date.as_deref().map(str::trim).filter(|d| !d.is_empty()).unwrap_or("");
    let rates = parsed.rates.unwrap_or_default();
    if rates.is_empty() {
        return Ok(Vec::new());
    }
    Ok(rates
        .into_iter()
        .map(|(currency, rate)| {
            let title = format!("{base} -> {currency}");
            let url = "https://www.frankfurter.app/docs/".to_string();
            let mut snippet = String::new();
            snippet.push_str(&format!("1 {base} = {rate} {currency}. "));
            if !date.is_empty() {
                snippet.push_str(&format!("Date: {date}. "));
            }
            build_result(ENGINE, &url, &title, &snippet)
        })
        .collect())
}

pub struct FrankfurterProvider {
    config: Config,
}

impl FrankfurterProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for FrankfurterProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        // Query is expected to be like "USD to EUR" or just "USD"; default target EUR.
        let parts: Vec<&str> = query.split_whitespace().collect();
        let base = parts.first().copied().unwrap_or("EUR").to_uppercase();
        let target = parts.get(2).or(parts.get(1)).copied().unwrap_or("USD").to_uppercase();
        let per_page = limit.clamp(1, 50);
        let response = api_client()
            .get(format!("{ENDPOINT}/latest"))
            .query(&[
                ("from", base.clone()),
                ("to", target.clone()),
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
            .get(format!("{ENDPOINT}/latest"))
            .query(&[("from", "EUR".to_string()), ("to", "USD".to_string())])
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
      "amount": 1,
      "base": "EUR",
      "date": "2024-01-15",
      "rates": { "USD": 1.09, "GBP": 0.86 }
    }"#;

    #[test]
    fn parses_rates() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|r| r.title == "EUR -> USD" && r.snippet.contains("1 EUR = 1.09 USD")));
        assert!(results.iter().any(|r| r.title == "EUR -> GBP" && r.snippet.contains("Date: 2024-01-15")));
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
