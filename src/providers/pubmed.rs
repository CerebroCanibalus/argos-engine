//! PubMed E-utilities adapter (keyless, `P1_VERTICAL`).
//!
//! PubMed is the biomedical literature index at the source, independent of
//! Europe PMC's aggregation. E-utilities has no key requirement for modest
//! traffic (an optional key raises the rate ceiling).
//!
//! Metadata needs two calls: `esearch` resolves terms to PMIDs, then `esummary`
//! turns those ids into titles, journals and dates. Both are public and
//! documented; the pair is kept sequential because `esummary` needs the ids.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{
    SearchProvider, api_client, build_result, map_api_status, offset_of, retry_after_of,
};

const ENGINE: &str = "pubmed";
const ESEARCH: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi";
const ESUMMARY: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esummary.fcgi";

#[derive(Deserialize, Debug, Default)]
struct RawSearch {
    #[serde(default)]
    esearchresult: RawSearchResult,
}

#[derive(Deserialize, Debug, Default)]
struct RawSearchResult {
    #[serde(default)]
    idlist: Vec<String>,
}

/// `esummary` returns `result` as a map of documents keyed by PMID, plus a
/// sibling `uids` entry holding the id list. Deserialising the whole map
/// straight into `RawDoc` fails on that array, so the envelope is read as
/// generic values and only the object entries are decoded.
#[derive(Deserialize, Debug, Default)]
struct RawSummary {
    #[serde(default)]
    result: std::collections::BTreeMap<String, flojo_mcp::serde_json::Value>,
}

#[derive(Deserialize, Debug, Default)]
struct RawDoc {
    #[serde(default)]
    title: String,
    #[serde(default)]
    pubdate: String,
    #[serde(default)]
    fulljournalname: String,
    #[serde(default)]
    elocationid: Option<String>,
    #[serde(default)]
    sortpubdate: String,
    #[serde(default)]
    authors: Vec<RawAuthor>,
}

#[derive(Deserialize, Debug)]
struct RawAuthor {
    #[serde(default)]
    name: String,
}

/// Prefer the DOI in `elocationid`, which PubMed formats as `doi:10.x/y`.
fn doi_from_elocation(raw: Option<&str>) -> Option<String> {
    let value = raw?.trim();
    let candidate = value
        .strip_prefix("doi:")
        .or_else(|| value.strip_prefix("DOI:"))
        .unwrap_or(value);
    // A locator like "1234-5678" is not a DOI; require the 10.x prefix.
    candidate.starts_with("10.").then(|| candidate.to_string())
}

/// Parse an `esummary` payload into compact results (pure).
pub fn parse_summary(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawSummary = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .result
        .into_iter()
        .filter_map(|(id, value)| {
            // Serde reads a JSON sequence into a struct happily, so the sibling
            // `uids` array would decode as a paper titled with its first id.
            if !value.is_object() {
                return None;
            }
            flojo_mcp::serde_json::from_value::<RawDoc>(value)
                .ok()
                .map(|doc| (id, doc))
        })
        .filter(|(_, doc)| !doc.title.is_empty())
        .map(|(id, doc)| {
            let title = doc.title.trim();
            let url = match doi_from_elocation(doc.elocationid.as_deref()) {
                Some(doi) => format!("https://doi.org/{doi}"),
                None => format!("https://pubmed.ncbi.nlm.nih.gov/{id}/"),
            };
            let authors = doc
                .authors
                .iter()
                .map(|a| a.name.trim())
                .filter(|name| !name.is_empty())
                .take(3)
                .collect::<Vec<_>>()
                .join(", ");
            let mut snippet = String::new();
            if !authors.is_empty() {
                snippet.push_str(&authors);
                snippet.push_str(". ");
            }
            let journal = doc.fulljournalname.trim();
            if !journal.is_empty() {
                snippet.push_str(journal);
                snippet.push_str(". ");
            }
            let date = if doc.pubdate.trim().is_empty() {
                doc.sortpubdate.trim()
            } else {
                doc.pubdate.trim()
            };
            if !date.is_empty() {
                snippet.push_str(date);
                snippet.push('.');
            }
            build_result(ENGINE, &url, title, &snippet)
        })
        .collect())
}

/// Extract PMIDs from an `esearch` payload (pure).
pub fn parse_ids(payload: &str) -> Result<Vec<String>, ArgosError> {
    let parsed: RawSearch = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .esearchresult
        .idlist
        .into_iter()
        .filter(|id| !id.trim().is_empty())
        .collect())
}

/// PubMed provider over the shared public-API HTTP client.
pub struct PubmedProvider {
    config: Config,
}

impl PubmedProvider {
    /// Create the provider for the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    fn common(&self, db: &str) -> Vec<(&'static str, String)> {
        let mut params = vec![
            ("db", db.to_string()),
            ("retmode", "json".to_string()),
            ("tool", "argos-engine".to_string()),
        ];
        if let Some(email) = &self.config.contact_email {
            params.push(("email", email.clone()));
        }
        params
    }
}

#[async_trait]
impl SearchProvider for PubmedProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let retmax = limit.clamp(1, 50);
        let start = offset_of(page, retmax);
        let mut params = self.common("pubmed");
        params.push(("term", query.to_string()));
        params.push(("retmax", retmax.to_string()));
        params.push(("retstart", start.to_string()));

        let search = api_client()
            .get(ESEARCH)
            .query(&params)
            .timeout(self.config.request_timeout)
            .send()
            .await
            .map_err(|ex| ArgosError::Unreachable {
                origin: ENGINE.into(),
                cause: ex.to_string(),
            })?;
        let status = search.status();
        if !status.is_success() {
            let code = status.as_u16();
            return Err(map_api_status(ENGINE, code, retry_after_of(&search)));
        }
        let ids = parse_ids(&search.text().await.map_err(|ex| ArgosError::Unreachable {
            origin: ENGINE.into(),
            cause: ex.to_string(),
        })?)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut summary_params = self.common("pubmed");
        summary_params.push(("id", ids.join(",")));
        let summary = api_client()
            .get(ESUMMARY)
            .query(&summary_params)
            .timeout(self.config.request_timeout)
            .send()
            .await
            .map_err(|ex| ArgosError::Unreachable {
                origin: ENGINE.into(),
                cause: ex.to_string(),
            })?;
        let status = summary.status();
        if !status.is_success() {
            let code = status.as_u16();
            return Err(map_api_status(ENGINE, code, retry_after_of(&summary)));
        }
        let mut results =
            parse_summary(&summary.text().await.map_err(|ex| ArgosError::Unreachable {
                origin: ENGINE.into(),
                cause: ex.to_string(),
            })?)?;
        results.truncate(limit);
        Ok(results)
    }

    async fn health(&self) -> bool {
        let mut params = self.common("pubmed");
        params.push(("term", "cancer".to_string()));
        params.push(("retmax", "1".to_string()));
        match api_client()
            .get(ESEARCH)
            .query(&params)
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

    const SEARCH: &str = r#"{
      "esearchresult": { "count": "1200", "idlist": ["111", "222", " "] }
    }"#;

    const SUMMARY: &str = r#"{
      "result": {
        "uids": ["111", "222", "333"],
        "111": {
          "title": "Gated recurrent units for greenhouse gas forecasting",
          "pubdate": "2026",
          "sortpubdate": "2026 01 15 10:00",
          "fulljournalname": "Frontiers in Artificial Intelligence",
          "elocationid": "doi:10.3389/frai.2026.1954403",
          "authors": [{"name": "Kimei EH"}, {"name": "Nyambo DG"}]
        },
        "222": {
          "title": "A record without a DOI falls back to PubMed",
          "pubdate": "2025",
          "fulljournalname": "Some Journal",
          "elocationid": "1234-5678",
          "authors": []
        },
        "333": { "title": "", "pubdate": "" }
      }
    }"#;

    #[test]
    fn parses_ids_and_skips_blanks() {
        assert_eq!(parse_ids(SEARCH).expect("ids"), vec!["111", "222"]);
        assert!(parse_ids("{\"esearchresult\": {}}").is_ok_and(|ids| ids.is_empty()));
    }

    #[test]
    fn summary_prefers_doi_and_skips_the_uids_list() {
        let results = parse_summary(SUMMARY).expect("summary");
        assert_eq!(results.len(), 2, "the untitled record is unusable");
        assert_eq!(results[0].url, "https://doi.org/10.3389/frai.2026.1954403");
        assert_eq!(results[1].url, "https://pubmed.ncbi.nlm.nih.gov/222/");
        assert!(results[0].snippet.starts_with("Kimei EH, Nyambo DG."));
        assert!(
            results[0]
                .snippet
                .contains("Frontiers in Artificial Intelligence")
        );
    }

    #[test]
    fn elocation_locator_is_not_mistaken_for_a_doi() {
        assert!(doi_from_elocation(Some("1234-5678")).is_none());
        assert_eq!(
            doi_from_elocation(Some("doi:10.1/x")).as_deref(),
            Some("10.1/x")
        );
        assert!(doi_from_elocation(None).is_none());
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_summary("not json").is_err());
        assert!(parse_ids("<html/>").is_err());
    }
}
