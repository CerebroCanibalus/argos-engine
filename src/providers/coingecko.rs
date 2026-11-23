//! CoinGecko adapter (`knowledge` profile).
//!
//! Public crypto market search, no key. Returns coin name, symbol, market cap
//! rank and a canonical coingecko.com URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "coingecko";
const ENDPOINT: &str = "https://api.coingecko.com/api/v3/search";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    coins: Vec<RawCoin>,
}

#[derive(Deserialize, Debug, Default)]
struct RawCoin {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default, rename = "market_cap_rank")]
    market_cap_rank: Option<i64>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .coins
        .into_iter()
        .filter_map(|coin| {
            let name = coin
                .name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())?;
            let symbol = coin
                .symbol
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("");
            let id = coin
                .id
                .as_deref()
                .map(str::trim)
                .filter(|i| !i.is_empty())
                .unwrap_or("");
            let url = if id.is_empty() {
                "https://www.coingecko.com".to_string()
            } else {
                format!("https://www.coingecko.com/en/coins/{id}")
            };
            let mut snippet = String::new();
            if !symbol.is_empty() {
                snippet.push_str(&format!("{symbol}. "));
            }
            if let Some(rank) = coin.market_cap_rank {
                snippet.push_str(&format!("Market cap rank: {rank}. "));
            }
            Some(build_result(ENGINE, &url, name, &snippet))
        })
        .collect())
}

pub struct CoinGeckoProvider {
    config: Config,
}

impl CoinGeckoProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for CoinGeckoProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let per_page = limit.clamp(1, 50);
        let response = api_client()
            .get(ENDPOINT)
            .query(&[("query", query.to_string())])
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
            .query(&[("query", "bitcoin".to_string())])
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
      "coins": [
        {
          "id": "bitcoin",
          "name": "Bitcoin",
          "symbol": "btc",
          "market_cap_rank": 1
        },
        { "name": "" }
      ]
    }"#;

    #[test]
    fn parses_coins_and_skips_nameless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Bitcoin");
        assert!(results[0].snippet.contains("btc."));
        assert!(results[0].snippet.contains("Market cap rank: 1."));
        assert_eq!(results[0].url, "https://www.coingecko.com/en/coins/bitcoin");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
