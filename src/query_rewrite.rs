//! Query rewriting: normalization, synonym expansion and intent detection.
//!
//! Keeps the search payload compact while widening recall: a query for
//! "rust async runtime" also reaches providers with "asynchronous runtime".

use std::collections::HashMap;

/// Known technical synonym groups. Keys are canonical; values are alternates.
fn synonyms() -> HashMap<&'static str, &'static [&'static str]> {
    let mut m = HashMap::new();
    m.insert("async", &["asynchronous"][..]);
    m.insert("runtime", &["engine", "executor"][..]);
    m.insert("vst", &["plugin", "instrument"][..]);
    m.insert("crate", &["package", "library"][..]);
    m.insert("npm", &["node", "javascript"][..]);
    m.insert("db", &["database"][..]);
    m.insert("auth", &["authentication", "authorization"][..]);
    m.insert("ml", &["machine learning"][..]);
    m.insert("ai", &["artificial intelligence"][..]);
    m.insert("ui", &["user interface"][..]);
    m.insert("api", &["endpoint", "interface"][..]);
    m
}

/// Normalize whitespace and casing-insensitive cleanup; preserve original case
/// for providers that are case-sensitive.
pub fn normalize(query: &str) -> String {
    query.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Expand a query with synonym alternates using OR grouping.
pub fn expand(query: &str) -> String {
    let synonyms = synonyms();
    let mut terms: Vec<String> = Vec::new();
    for token in query.split_whitespace() {
        let lower = token.to_ascii_lowercase();
        if let Some(alts) = synonyms.get(lower.as_str()) {
            let group = std::iter::once(token.to_string())
                .chain(alts.iter().map(|a| (*a).to_string()))
                .collect::<Vec<_>>()
                .join(" OR ");
            terms.push(format!("({group})"));
        } else {
            terms.push(token.to_string());
        }
    }
    terms.join(" ")
}

/// Detect a coarse intent from the query. Used to bias provider selection in
/// the future; for now it is informational.
#[allow(dead_code)]
pub fn intent(query: &str) -> &'static str {
    let lower = query.to_ascii_lowercase();
    if lower.contains("how to") || lower.contains("how do") {
        "howto"
    } else if lower.contains("crate") || lower.contains("package") || lower.contains("library") {
        "package"
    } else if lower.contains("paper") || lower.contains("study") || lower.contains("research") {
        "academic"
    } else if lower.contains("news") || lower.contains("latest") || lower.contains("update") {
        "news"
    } else {
        "general"
    }
}

/// Full rewrite: normalize + expand. Returns the query to send upstream.
pub fn rewrite(query: &str) -> String {
    expand(&normalize(query))
}
