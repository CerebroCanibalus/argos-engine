//! Europe PMC REST adapter (keyless, `P1_VERTICAL`).
//!
//! Europe PMC indexes PubMed, PMC and preprints behind one documented REST
//! API that needs no key. It is the highest-precision biomedical source in the
//! keyless set and overlaps partially with the `pubmed` adapter, which is
//! useful rather than wasteful: agreement between two independent indexes is
//! exactly what RRF rewards.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{
    SearchProvider, api_client, build_result, map_api_status, offset_of, retry_after_of,
};

const ENGINE: &str = "europe_pmc";
const ENDPOINT: &str = "https://www.ebi.ac.uk/europepmc/webservices/rest/search";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(rename = "resultList", default)]
    result_list: RawResultList,
}

#[derive(Deserialize, Debug, Default)]
struct RawResultList {
    #[serde(default)]
    result: Vec<RawResult>,
}

#[derive(Deserialize, Debug, Default)]
struct RawResult {
    #[serde(default)]
    id: String,
    /// `MED` for PubMed, `PMC`, `AGR` etc. Needed to build an EPMC link.
    #[serde(default)]
    source: String,
    #[serde(default)]
    doi: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(rename = "authorString", default)]
    author_string: Option<String>,
    #[serde(rename = "journalTitle", default)]
    journal_title: Option<String>,
    #[serde(rename = "pubYear", default)]
    pub_year: Option<String>,
    /// Only present with `resultType=core`.
    #[serde(rename = "abstractText", default)]
    abstract_text: Option<String>,
}

/// Stable landing page: the DOI resolver when known, else the Europe PMC record.
fn canonical_url(result: &RawResult) -> Option<String> {
    if let Some(doi) = result
        .doi
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        return Some(format!("https://doi.org/{doi}"));
    }
    let id = result.id.trim();
    if id.is_empty() {
        return None;
    }
    let source = result.source.trim();
    Some(if source.is_empty() {
        format!("https://europepmc.org/article/{id}")
    } else {
        format!("https://europepmc.org/article/{source}/{id}")
    })
}

/// Parse a Europe PMC `search` payload into compact results (pure).
pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .result_list
        .result
        .into_iter()
        .filter_map(|record| {
            let url = canonical_url(&record)?;
            let title = record.title.as_deref().unwrap_or_default().trim();
            if title.is_empty() {
                return None;
            }
            let mut snippet = String::new();
            if let Some(authors) = record
                .author_string
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())
            {
                snippet.push_str(authors);
                snippet.push_str(". ");
            }
            if let Some(journal) = record
                .journal_title
                .as_deref()
                .map(str::trim)
                .filter(|j| !j.is_empty())
            {
                snippet.push_str(&format!("{journal}. "));
            }
            if let Some(year) = record
                .pub_year
                .as_deref()
                .map(str::trim)
                .filter(|y| !y.is_empty())
            {
                snippet.push_str(&format!("{year}. "));
            }
            if let Some(abstract_text) = record
                .abstract_text
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())
            {
                snippet.push_str(abstract_text);
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

/// Europe PMC provider over the shared public-API HTTP client.
pub struct EuropePmcProvider {
    config: Config,
}

impl EuropePmcProvider {
    /// Create the provider for the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for EuropePmcProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let page_size = limit.clamp(1, 50);
        let offset = offset_of(page, page_size);
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("query", query.to_string()),
                ("format", "json".to_string()),
                // `core` is required for `abstractText`; `lite` omits it.
                ("resultType", "core".to_string()),
                ("pageSize", page_size.to_string()),
                ("offset", offset.to_string()),
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
                ("query", "cancer".to_string()),
                ("format", "json".to_string()),
                ("pageSize", "1".to_string()),
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
      "hitCount": 2,
      "resultList": { "result": [
        {
          "id": "42694560",
          "source": "MED",
          "doi": "10.3389/frai.2026.1954403",
          "title": "Gated recurrent unit model for forecasting greenhouse gas concentrations.",
          "authorString": "Kimei EH, Nyambo DG.",
          "journalTitle": "Front Artif Intell",
          "pubYear": "2026",
          "abstractText": "We evaluate a gated recurrent unit model."
        },
        {
          "id": "999",
          "source": "MED",
          "doi": "",
          "title": "Record without DOI uses the EPMC link",
          "authorString": "Someone A."
        },
        { "id": "", "title": "No id is unusable" }
      ] }
    }"#;

    #[test]
    fn parses_europepmc_contract() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 2, "records without ids must be dropped");
        assert_eq!(results[0].url, "https://doi.org/10.3389/frai.2026.1954403");
        assert_eq!(results[0].source, "doi.org");
        assert_eq!(results[1].url, "https://europepmc.org/article/MED/999");
    }

    #[test]
    fn snippet_carries_authors_venue_year_and_abstract() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        let snippet = &results[0].snippet;
        assert!(snippet.starts_with("Kimei EH, Nyambo DG. Front Artif Intell. 2026."));
        assert!(snippet.ends_with("We evaluate a gated recurrent unit model."));
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("<html>down</html>").is_err());
        assert!(parse_results("{\"resultList\": {}}").is_ok_and(|r| r.is_empty()));
    }

    #[test]
    fn paging_is_one_based() {
        assert_eq!(offset_of(1, 10), 0);
        assert_eq!(offset_of(3, 10), 20);
        assert_eq!(offset_of(0, 10), 0);
    }
}
