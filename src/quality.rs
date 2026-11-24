//! Quality gates for provider results: validated domain scope and query relevance.
//!
//! Upstream search pages are not a contract. A provider can return HTTP 200
//! with a challenge, parser drift, or a SERP for a different intent. These
//! checks are deliberately conservative: they remove obvious scope leaks and
//! results with no meaningful query term, while the fanout reports what was
//! removed instead of pretending that the page was useful.

use std::collections::HashSet;

use crate::error::ArgosError;
use crate::types::SearchResult;

const MAX_DOMAINS: usize = 8;
const MIN_TERM_CHARS: usize = 2;

/// Validate and normalize an optional domain allowlist.
///
/// Domains are host suffixes, not arbitrary query fragments. Rejecting query
/// syntax here prevents `domains` from becoming an accidental query injection
/// surface when the value is later rendered as `site:` operators.
pub fn normalize_domains(raw: Option<&[String]>) -> Result<Vec<String>, ArgosError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    if raw.len() > MAX_DOMAINS {
        return Err(ArgosError::InvalidQuery(format!(
            "domains accepts at most {MAX_DOMAINS} entries"
        )));
    }

    let mut normalized = Vec::new();
    let mut seen = HashSet::new();
    for value in raw {
        let domain = value.trim().trim_end_matches('.').to_ascii_lowercase();
        let valid = !domain.is_empty()
            && domain.contains('.')
            && !domain.starts_with('.')
            && !domain.ends_with('.')
            && !domain.contains("..")
            && domain
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
        if !valid {
            return Err(ArgosError::InvalidQuery(format!(
                "invalid domain scope: {value:?}; pass a hostname such as kvrforums.com"
            )));
        }
        if seen.insert(domain.clone()) {
            normalized.push(domain);
        }
    }
    Ok(normalized)
}

/// Append validated `site:` operators for providers that understand them.
pub fn scoped_query(query: &str, domains: &[String]) -> String {
    if domains.is_empty() {
        return query.to_string();
    }
    let sites = domains
        .iter()
        .map(|domain| format!("site:{domain}"))
        .collect::<Vec<_>>()
        .join(" OR ");
    format!("{query} ({sites})")
}

/// Whether a result URL belongs to one of the requested host suffixes.
pub fn matches_domains(url: &str, domains: &[String]) -> bool {
    if domains.is_empty() {
        return true;
    }
    let Some(host) = host_for(url) else {
        return false;
    };
    domains.iter().any(|domain| {
        host == *domain
            || host
                .strip_suffix(domain)
                .is_some_and(|prefix| prefix.ends_with('.'))
    })
}

/// Partial relevance score (0.0-1.0) for a result against a query.
///
/// Combines exact term matches with synonym alternates. Title matches weigh
/// more than snippet/url matches. Returns 0.0 when nothing matches.
pub fn relevance_score(query: &str, result: &SearchResult) -> f64 {
    let terms = meaningful_terms(query);
    if terms.is_empty() {
        return 1.0;
    }
    let title = result.title.to_ascii_lowercase();
    let snippet = result.snippet.to_ascii_lowercase();
    let url = result.url.to_ascii_lowercase();

    let mut total = 0.0;
    for term in &terms {
        let alternates = crate::query_rewrite::synonym_alternates(term);
        // Exact term first.
        if title.contains(term.as_str()) {
            total += 0.6;
        } else if snippet.contains(term.as_str()) || url.contains(term.as_str()) {
            total += 0.3;
        } else {
            // Synonym alternates (lower weight).
            for alt in &alternates {
                let alt = alt.to_ascii_lowercase();
                if title.contains(&alt) {
                    total += 0.4;
                    break;
                } else if snippet.contains(&alt) || url.contains(&alt) {
                    total += 0.2;
                    break;
                }
            }
        }
    }
    (total / terms.len() as f64).min(1.0)
}

/// Conservative lexical relevance gate for a normalized result.
///
/// A result is rejected only when it has zero relevance (no term or synonym
/// matched). This is far more permissive than the old binary gate while still
/// filtering obvious entity drift.
pub fn is_relevant(query: &str, result: &SearchResult) -> bool {
    relevance_score(query, result) > 0.0
}

fn host_for(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(|host| host.to_ascii_lowercase()))
}

fn meaningful_terms(query: &str) -> Vec<String> {
    const GENERIC: &[&str] = &[
        "a", "an", "and", "are", "as", "at", "be", "best", "by", "for", "free", "from", "how",
        "in", "into", "is", "it", "of", "on", "or", "review", "the", "to", "top", "tutorial",
        "with",
    ];
    let mut seen = HashSet::new();
    query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| term.chars().count() >= MIN_TERM_CHARS)
        .map(|term| term.to_ascii_lowercase())
        .filter(|term| !GENERIC.contains(&term.as_str()))
        .filter(|term| seen.insert(term.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(url: &str, title: &str, snippet: &str) -> SearchResult {
        SearchResult {
            url: url.into(),
            source: String::new(),
            title: title.into(),
            snippet: snippet.into(),
            engine: Some("test".into()),
            published_at: None,
}
    }

    #[test]
    fn normalizes_and_deduplicates_domains() {
        let raw = vec![" KVRForums.com. ".into(), "kvrforums.com".into()];
        assert_eq!(
            normalize_domains(Some(&raw)).expect("valid domains"),
            vec!["kvrforums.com"]
        );
    }

    #[test]
    fn rejects_query_syntax_in_domains() {
        let raw = vec!["example.com) OR (site:other.test".into()];
        assert!(normalize_domains(Some(&raw)).is_err());
    }

    #[test]
    fn scope_accepts_exact_host_and_subdomains_only() {
        let domains = vec!["reddit.com".into()];
        assert!(matches_domains("https://reddit.com/r/test", &domains));
        assert!(matches_domains("https://www.reddit.com/r/test", &domains));
        assert!(!matches_domains("https://notreddit.com/r/test", &domains));
    }

    #[test]
    fn relevance_rejects_entity_drift() {
        let query = "free piano VST";
        assert!(!is_relevant(
            query,
            &result("https://poki.com", "Free Online Games", "Play free games")
        ));
        assert!(is_relevant(
            query,
            &result("https://kvr.test", "Free piano VST", "Piano instrument")
        ));
    }
}
