//! arXiv public Atom API provider (keyless, `P1_PUBLIC_API`).
//!
//! arXiv is the reference open preprint server for physics, maths, CS and
//! quantitative biology. It publishes a documented Atom endpoint that needs no
//! key. The one caveat is politeness: their user manual asks for no more than
//! one request every three seconds, so paging honours the offset explicitly
//! instead of asking for a deep result set.

use flojo_mcp::async_trait::async_trait;
use flojo_mcp::truncate_string;
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::config::Config;
use crate::error::ArgosError;
use crate::limits::{SNIPPET_MAX_CHARS, TITLE_MAX_CHARS};
use crate::providers::{SearchProvider, api_client, compact_source, parse_retry_after};
use crate::types::SearchResult;

const ENGINE: &str = "arxiv";
const ENDPOINT: &str = "https://export.arxiv.org/api/query";
const PER_PAGE_MAX: usize = 50;

/// One `<entry>` while it is being read.
#[derive(Default)]
struct EntryBuilder {
    id: String,
    title: String,
    summary: String,
    published: String,
    authors: Vec<String>,
}

/// Which element the reader is currently collecting text for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Capture {
    None,
    Id,
    Title,
    Summary,
    Published,
    Author,
}

/// Convert an arXiv abstract link to its stable `https` abs page.
fn canonical_arxiv_url(id: &str) -> String {
    id.trim()
        .replace("http://", "https://")
        .trim_end_matches('/')
        .to_string()
}

/// Year only, which is all that fits the snippet budget.
fn year_of(published: &str) -> Option<&str> {
    published
        .get(..4)
        .filter(|year| year.chars().all(|c| c.is_ascii_digit()))
}

/// Parse an arXiv Atom feed into compact results (pure, fixture-testable).
pub fn parse_results(xml: &str) -> Result<Vec<SearchResult>, ArgosError> {
    if !xml.contains("<feed") {
        return Err(ArgosError::Decode("not an arXiv Atom feed".into()));
    }

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut entries: Vec<EntryBuilder> = Vec::new();
    let mut current: Option<EntryBuilder> = None;
    let mut capture = Capture::None;
    let mut buffer = Vec::new();

    loop {
        match reader.read_event_into(&mut buffer) {
            Err(ex) => return Err(ArgosError::Decode(ex.to_string())),
            Ok(Event::Eof) => break,
            Ok(Event::Start(element)) => {
                let name = element.name();
                match name.as_ref() {
                    b"entry" => current = Some(EntryBuilder::default()),
                    b"id" => capture = Capture::Id,
                    b"title" => capture = Capture::Title,
                    b"summary" => capture = Capture::Summary,
                    b"published" => capture = Capture::Published,
                    b"name" => capture = Capture::Author,
                    _ => {}
                }
            }
            // Author names are self-closing-empty in some feeds.
            Ok(Event::Empty(element)) => {
                let name = element.name();
                if name.as_ref() == b"name" {
                    capture = Capture::Author;
                }
            }
            Ok(Event::Text(text)) => {
                if capture != Capture::None {
                    let decoded = text.unescape().unwrap_or_default();
                    let chunk = decoded.trim().to_string();
                    if !chunk.is_empty()
                        && let Some(entry) = current.as_mut()
                    {
                        match capture {
                            Capture::Id => entry.id.push_str(&chunk),
                            Capture::Title => {
                                if !entry.title.is_empty() {
                                    entry.title.push(' ');
                                }
                                entry.title.push_str(&chunk);
                            }
                            Capture::Summary => entry.summary.push_str(&chunk),
                            Capture::Published => entry.published.push_str(&chunk),
                            Capture::Author => {
                                entry.authors.push(chunk);
                                capture = Capture::None;
                            }
                            Capture::None => {}
                        }
                    }
                }
            }
            Ok(Event::CData(text)) => {
                if capture != Capture::None
                    && let Some(entry) = current.as_mut()
                {
                    let chunk = String::from_utf8_lossy(text.as_ref()).trim().to_string();
                    if !chunk.is_empty() {
                        match capture {
                            Capture::Id => entry.id.push_str(&chunk),
                            Capture::Title => {
                                if !entry.title.is_empty() {
                                    entry.title.push(' ');
                                }
                                entry.title.push_str(&chunk);
                            }
                            Capture::Summary => entry.summary.push_str(&chunk),
                            Capture::Published => entry.published.push_str(&chunk),
                            Capture::Author => {
                                entry.authors.push(chunk);
                                capture = Capture::None;
                            }
                            Capture::None => {}
                        }
                    }
                }
            }
            Ok(Event::End(element)) => {
                let name = element.name();
                if name.as_ref() == b"entry"
                    && let Some(entry) = current.take()
                {
                    entries.push(entry);
                }
                match name.as_ref() {
                    b"id" | b"title" | b"summary" | b"published" | b"name" => {
                        capture = Capture::None;
                    }
                    _ => {}
                }
            }
            Ok(_) => {}
        }
        buffer.clear();
    }

    Ok(entries
        .into_iter()
        .filter_map(|entry| {
            let url = canonical_arxiv_url(&entry.id);
            // Atom wraps long titles/summaries across lines, so internal
            // newlines and indentation must be folded away.
            let title = entry.title.split_whitespace().collect::<Vec<_>>().join(" ");
            if url.is_empty() || title.is_empty() {
                return None;
            }
            let authors = entry
                .authors
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");

            let mut snippet = String::new();
            if !authors.is_empty() {
                if let Some(year) = year_of(&entry.published) {
                    snippet.push_str(&format!("{authors} ({year}). "));
                } else {
                    snippet.push_str(&format!("{authors}. "));
                }
            }
            let summary = entry
                .summary
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            snippet.push_str(&summary);

            Some(SearchResult {
                source: compact_source(&url),
                url,
                title: truncate_string(&title, TITLE_MAX_CHARS).0,
                snippet: truncate_string(snippet.trim(), SNIPPET_MAX_CHARS).0,
                engine: Some(ENGINE.into()),
            })
        })
        .collect())
}

/// arXiv provider over the shared public-API HTTP client.
pub struct ArxivProvider {
    config: Config,
}

impl ArxivProvider {
    /// Create the provider for the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SearchProvider for ArxivProvider {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<SearchResult>, ArgosError> {
        let max_results = limit.clamp(1, PER_PAGE_MAX);
        let start = page.saturating_sub(1).saturating_mul(max_results);
        let response = api_client()
            .get(ENDPOINT)
            .query(&[
                ("search_query", format!("all:{query}")),
                ("start", start.to_string()),
                ("max_results", max_results.to_string()),
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
            if code == 429 || code == 403 {
                let retry_after_secs = response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok())
                    .and_then(parse_retry_after);
                return Err(ArgosError::RateLimited {
                    engine: ENGINE.into(),
                    status: code,
                    retry_after_secs,
                });
            }
            return Err(ArgosError::Unreachable {
                origin: ENGINE.into(),
                cause: format!("HTTP {code}"),
            });
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
            .query(&[("search_query", "all:electron"), ("max_results", "1")])
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

    const FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>arXiv Query</title>
  <id>http://arxiv.org/api/query</id>
  <entry>
    <id>http://arxiv.org/abs/2301.00001v1</id>
    <published>2023-01-01T00:00:00Z</published>
    <title>A Structured Study of
      Attention</title>
    <summary>We present a study of
      retrieval augmented generation.</summary>
    <author><name>Ada Lovelace</name></author>
    <author><name>Alan Turing</name></author>
  </entry>
  <entry>
    <id>https://arxiv.org/abs/2302.00002</id>
    <published>2023-02-01T00:00:00Z</published>
    <title>Second Entry</title>
    <summary>Another abstract.</summary>
  </entry>
  <entry>
    <id></id>
    <title>No id is unusable</title>
    <summary>dropped</summary>
  </entry>
</feed>
"#;

    #[test]
    fn parses_atom_entries() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        // The third entry has no id and is dropped.
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://arxiv.org/abs/2301.00001v1");
        assert_eq!(results[0].source, "arxiv.org");
        assert_eq!(results[0].engine.as_deref(), Some(ENGINE));
    }

    #[test]
    fn folds_whitespace_and_keeps_author_context() {
        let results = parse_results(FIXTURE).expect("fixture must parse");
        assert_eq!(results[0].title, "A Structured Study of Attention");
        assert!(
            results[0]
                .snippet
                .starts_with("Ada Lovelace, Alan Turing (2023)."),
            "snippet must carry authors and year: {}",
            results[0].snippet
        );
        assert!(
            results[0]
                .snippet
                .ends_with("retrieval augmented generation.")
        );
        // Second entry has no authors, so no dangling author prefix.
        assert!(results[1].snippet.starts_with("Another abstract."));
    }

    #[test]
    fn rejects_malformed_xml() {
        assert!(parse_results("<html>rate limited</html>").is_err());
        assert!(parse_results("").is_err());
    }

    #[test]
    fn canonical_url_normalizes_scheme_and_slash() {
        assert_eq!(
            canonical_arxiv_url("http://arxiv.org/abs/1234.5678v2"),
            "https://arxiv.org/abs/1234.5678v2"
        );
        assert_eq!(canonical_arxiv_url("  "), "");
    }
}
