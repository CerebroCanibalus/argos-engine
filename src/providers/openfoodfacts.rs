//! Open Food Facts adapter (`knowledge` profile).
//!
//! Public food product search, no key. Returns product name, brand, nutriscore
//! and a canonical openfoodfacts.org URL.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "openfoodfacts";
const ENDPOINT: &str = "https://world.openfoodfacts.org/cgi/search.pl";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    products: Vec<RawProduct>,
}

#[derive(Deserialize, Debug, Default)]
struct RawProduct {
    #[serde(default, rename = "product_name")]
    product_name: Option<String>,
    #[serde(default)]
    brands: Option<String>,
    #[serde(default, rename = "nutriscore_grade")]
    nutriscore_grade: Option<String>,
    #[serde(default, rename = "code")]
    code: Option<String>,
    #[serde(default, rename = "categories")]
    categories: Option<String>,
}

pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .products
        .into_iter()
        .filter_map(|p| {
            let title = p
                .product_name
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let code = p.code.as_deref().map(str::trim).filter(|c| !c.is_empty());
            let url = code
                .map(|c| format!("https://world.openfoodfacts.org/product/{c}"))
                .unwrap_or_else(|| "https://world.openfoodfacts.org".to_string());
            let mut snippet = String::new();
            if let Some(brand) = p.brands.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
                snippet.push_str(&format!("{brand}. "));
            }
            if let Some(grade) = p
                .nutriscore_grade
                .as_deref()
                .map(str::trim)
                .filter(|g| !g.is_empty())
            {
                snippet.push_str(&format!("Nutri-Score: {}. ", grade.to_uppercase()));
            }
            if let Some(categories) = p
                .categories
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
            {
                snippet.push_str(&format!("Categories: {}. ", categories));
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

pub struct OpenFoodFactsProvider {
    config: Config,
}

impl OpenFoodFactsProvider {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for OpenFoodFactsProvider {
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
                ("search_terms", query.to_string()),
                ("search_simple", "1".to_string()),
                ("action", "process".to_string()),
                ("json", "1".to_string()),
                ("page_size", per_page.to_string()),
                ("page", page.to_string()),
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
                ("search_terms", "chocolate".to_string()),
                ("search_simple", "1".to_string()),
                ("action", "process".to_string()),
                ("json", "1".to_string()),
                ("page_size", "1".to_string()),
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
      "products": [
        {
          "product_name": "Dark Chocolate 70%",
          "brands": "Lindt",
          "nutriscore_grade": "b",
          "code": "3046920029759",
          "categories": "Chocolates,Dark chocolates",
          "image_url": "https://images.openfoodfacts.org/x.jpg"
        },
        { "product_name": "" }
      ]
    }"#;

    #[test]
    fn parses_products_and_skips_nameless() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Dark Chocolate 70%");
        assert!(results[0].snippet.contains("Lindt."));
        assert!(results[0].snippet.contains("Nutri-Score: B."));
        assert!(
            results[0]
                .snippet
                .contains("Categories: Chocolates,Dark chocolates.")
        );
        assert_eq!(
            results[0].url,
            "https://world.openfoodfacts.org/product/3046920029759"
        );
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("nope").is_err());
    }
}
