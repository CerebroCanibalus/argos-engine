//! DOAJ (Directory of Open Access Journals) adapter (keyless, `P1_PUBLIC_API`).
//!
//! DOAJ indexes open-access peer-reviewed journals and exposes a documented
//! public search API with no key. It is the open-access slice of the literature,
//! so it complements rather than duplicates OpenAlex or Crossref.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::serde::Deserialize;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::{SearchProvider, api_client, build_result, map_api_status, retry_after_of};

const ENGINE: &str = "doaj";
const ENDPOINT: &str = "https://doaj.org/api/search/articles";

#[derive(Deserialize, Debug, Default)]
struct RawResponse {
    #[serde(default)]
    results: Vec<RawArticle>,
}

#[derive(Deserialize, Debug, Default)]
struct RawArticle {
    #[serde(default)]
    bibjson: BibJson,
}

#[derive(Deserialize, Debug, Default)]
struct BibJson {
    #[serde(default)]
    identifier: Vec<Identifier>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    year: Option<String>,
    #[serde(default)]
    author: Vec<Author>,
    #[serde(default)]
    journal: Journal,
    #[serde(default)]
    keywords: Vec<String>,
}

#[derive(Deserialize, Debug)]
struct Identifier {
    #[serde(default)]
    id: String,
    #[serde(rename = "type", default)]
    id_type: String,
}

#[derive(Deserialize, Debug)]
struct Author {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize, Debug, Default)]
struct Journal {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    publisher: Option<String>,
}

/// Find the article's DOI, falling back to the journal's ISSN for a stable link.
fn canonical_url(article: &BibJson) -> Option<String> {
    for wanted in ["doi", "eissn", "pissn", "issn"] {
        if let Some(found) = article.identifier.iter().find(|i| i.id_type == wanted) {
            let id = found.id.trim();
            if id.is_empty() {
                continue;
            }
            return Some(if wanted == "doi" {
                format!("https://doi.org/{id}")
            } else {
                format!("https://doaj.org/article/{id}")
            });
        }
    }
    None
}

/// Parse a DOAJ `search/articles` payload into compact results (pure).
pub fn parse_results(payload: &str) -> Result<Vec<super::SearchResult>, ArgosError> {
    let parsed: RawResponse = flojo_mcp::serde_json::from_str(payload)
        .map_err(|ex| ArgosError::Decode(ex.to_string()))?;
    Ok(parsed
        .results
        .into_iter()
        .filter_map(|article| {
            let bibjson = article.bibjson;
            let url = canonical_url(&bibjson)?;
            let title = bibjson.title.as_deref().unwrap_or_default().trim();
            if title.is_empty() {
                return None;
            }
            let mut snippet = String::new();
            let authors = bibjson
                .author
                .iter()
                .map(|a| a.name.trim())
                .filter(|name| !name.is_empty())
                .take(3)
                .collect::<Vec<_>>()
                .join(", ");
            if !authors.is_empty() {
                snippet.push_str(&authors);
                snippet.push_str(". ");
            }
            if let Some(journal) = bibjson
                .journal
                .title
                .as_deref()
                .map(str::trim)
                .filter(|j| !j.is_empty())
            {
                snippet.push_str(&format!("{journal}. "));
            }
            if let Some(publisher) = bibjson
                .journal
                .publisher
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
            {
                snippet.push_str(&format!("{publisher}. "));
            }
            if let Some(year) = bibjson
                .year
                .as_deref()
                .map(str::trim)
                .filter(|y| !y.is_empty())
            {
                snippet.push_str(&format!("{year}. "));
            }
            if !bibjson.keywords.is_empty() {
                let keywords = bibjson
                    .keywords
                    .iter()
                    .map(|k| k.trim())
                    .filter(|k| !k.is_empty())
                    .take(4)
                    .collect::<Vec<_>>()
                    .join(", ");
                if !keywords.is_empty() {
                    snippet.push_str(&keywords);
                    snippet.push('.');
                }
            }
            Some(build_result(ENGINE, &url, title, &snippet))
        })
        .collect())
}

/// DOAJ provider over the shared public-API HTTP client.
pub struct DoajProvider {
    config: Config,
}

impl DoajProvider {
    /// Create the provider for the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for DoajProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<super::SearchResult>, ArgosError> {
        let page_size = limit.clamp(1, 50);
        let current_page = page.max(1);
        let response = api_client()
            .get(format!("{ENDPOINT}/{}", urlencode_path(query)))
            .query(&[
                ("pageSize", page_size.to_string()),
                ("page", current_page.to_string()),
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
            .get(format!("{ENDPOINT}/{}", urlencode_path("cancer")))
            .query(&[("pageSize", "1"), ("page", "1")])
            .timeout(self.config.ping_timeout)
            .send()
            .await
        {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }
}

/// Percent-encode a query for use as one path segment.
///
/// DOAJ carries the search terms in the path rather than a query parameter, so
/// a query containing `/` or `?` would otherwise reshape the route.
fn urlencode_path(raw: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push_str("%20"),
            other => {
                out.push('%');
                out.push(HEX[(other >> 4) as usize] as char);
                out.push(HEX[(other & 0x0f) as usize] as char);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "total": 1699,
      "page": 1,
      "results": [
        {
          "bibjson": {
            "identifier": [
              {"id": "2169-3536", "type": "eissn"},
              {"id": "10.1109/ACCESS.2026.3671870", "type": "doi"}
            ],
            "title": "Retrieval augmented generation in the classroom",
            "journal": {"title": "IEEE Access", "publisher": "IEEE"},
            "year": "2026",
            "keywords": ["generative AI", "knowledge graph"],
            "author": [{"name": "Ada Lovelace"}, {"name": "Alan Turing"}, {"name": "Third"}, {"name": "Fourth"}]
          }
        },
        {
          "bibjson": {
            "identifier": [{"id": "1234-5678", "type": "eissn"}],
            "title": "Journal without a DOI",
            "year": "2025"
          }
        },
        {
          "bibjson": { "identifier": [], "title": "No identifier is unusable" }
        }
      ]
    }"#;

    #[test]
    fn parses_doaj_contract() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results.len(), 2, "records without identifiers are unusable");
        assert_eq!(
            results[0].url,
            "https://doi.org/10.1109/ACCESS.2026.3671870"
        );
        assert_eq!(results[1].url, "https://doaj.org/article/1234-5678");
    }

    #[test]
    fn snippet_caps_authors_and_keeps_keywords() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        let snippet = &results[0].snippet;
        assert!(snippet.starts_with("Ada Lovelace, Alan Turing, Third."));
        assert!(!snippet.contains("Fourth"), "author list must be capped");
        assert!(snippet.contains("IEEE Access"));
        assert!(snippet.ends_with("generative AI, knowledge graph."));
    }

    #[test]
    fn encodes_path_segments() {
        assert_eq!(urlencode_path("open access"), "open%20access");
        assert_eq!(urlencode_path("a/b?c"), "a%2Fb%3Fc");
        assert_eq!(urlencode_path("plain-Text_1.0~"), "plain-Text_1.0~");
    }

    #[test]
    fn rejects_invalid_payload() {
        assert!(parse_results("not json").is_err());
        assert!(parse_results("{\"results\": []}").is_ok_and(|r| r.is_empty()));
    }
}
