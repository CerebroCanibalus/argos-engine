use flojo_mcp::prelude::*;

use crate::config::Config;
use crate::error::ArgosError;

const DEFAULT_EXTRACT_CHARS: usize = 4000;
const MAX_EXTRACT_CHARS: usize = 12000;
const EXTRACT_CACHE_TTL_SECS: u64 = 300;

#[derive(serde::Serialize, Clone, Debug)]
pub struct ExtractOutcome {
    pub url: String,
    pub title: String,
    pub text: String,
    pub truncated: bool,
    pub cached: bool,
}

/// Process-local cache for extracted pages.
mod cache {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    #[derive(Clone)]
    pub struct Entry {
        pub title: String,
        pub text: String,
        pub expires_at: Instant,
    }

    static CACHE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();

    pub fn get(url: &str) -> Option<(String, String)> {
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let mut guard = cache.lock().ok()?;
        let now = Instant::now();
        if let Some(entry) = guard.get(url) {
            if entry.expires_at > now {
                return Some((entry.title.clone(), entry.text.clone()));
            }
            guard.remove(url);
        }
        None
    }

    pub fn put(url: &str, title: String, text: String) {
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        if let Ok(mut guard) = cache.lock() {
            guard.insert(
                url.to_string(),
                Entry {
                    title,
                    text,
                    expires_at: Instant::now() + Duration::from_secs(super::EXTRACT_CACHE_TTL_SECS),
                },
            );
        }
    }
}

fn extract_text(html: &str) -> (String, String) {
    use scraper::{Html, Selector};

    let document = Html::parse_document(html);

    // Title: prefer og:title, then <title>.
    let title = document
        .select(&Selector::parse("meta[property='og:title']").unwrap())
        .next()
        .and_then(|e| e.value().attr("content"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            document
                .select(&Selector::parse("title").unwrap())
                .next()
                .map(|e| e.text().collect::<String>().trim().to_string())
        })
        .unwrap_or_default();

    // Noise elements to skip entirely.
    let skip_tags = [
        "script", "style", "nav", "header", "footer", "aside", "noscript", "iframe", "form",
        "button", "svg", "canvas",
    ];

    // Pick the best content container: prefer semantic article/main, then common
    // content class/id selectors, finally body.
    let container_selectors = [
        "article",
        "main",
        "[role='main']",
        ".post-content",
        ".entry-content",
        ".article-content",
        ".article-body",
        "#content",
        ".content",
        ".post",
        "body",
    ];

    let mut best_text = String::new();
    let mut best_score: usize = 0;
    for sel in container_selectors {
        if let Ok(selector) = Selector::parse(sel) {
            for el in document.select(&selector) {
                let mut buf = String::new();
                collect_text(el, &skip_tags, &mut buf);
                // Score: number of word-like tokens; penalize link-heavy blocks.
                let words = buf.split_whitespace().count();
                let links = el.select(&Selector::parse("a").unwrap()).count();
                let score = words.saturating_sub(links * 3);
                if score > best_score && words >= 30 {
                    best_score = score;
                    best_text = buf;
                }
            }
        }
    }

    if best_text.trim().is_empty() {
        // Fallback: whole body.
        if let Some(body) = document.select(&Selector::parse("body").unwrap()).next() {
            collect_text(body, &skip_tags, &mut best_text);
        } else {
            collect_text(document.root_element(), &skip_tags, &mut best_text);
        }
    }

    // Deduplicate consecutive identical lines and drop tiny/noise lines.
    let mut lines: Vec<String> = Vec::new();
    let mut prev = String::new();
    for raw_line in best_text.lines() {
        let line = raw_line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            continue;
        }
        // Skip lines that are just punctuation or too short (likely nav).
        let word_count = line.split_whitespace().count();
        if word_count < 3 && line.len() < 40 {
            continue;
        }
        // Skip lines that are almost entirely links (high slash/dot density).
        if line.matches("http").count() > 3 {
            continue;
        }
        if line == prev {
            continue;
        }
        prev = line.clone();
        lines.push(line);
    }

    let mut out = lines.join("
");
    // Collapse multiple blank lines.
    while out.contains("


") {
        out = out.replace("


", "

");
    }
    let collapsed = out.trim().to_string();
    (title, collapsed)
}

fn collect_text(node: scraper::ElementRef, skip: &[&str], out: &mut String) {
    let name = node.value().name();
    if skip.contains(&name) {
        return;
    }
    // Block-level elements get a newline separator
    let block = matches!(
        name,
        "p" | "div" | "section" | "article" | "li" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "br" | "tr"
    );
    if block && !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    for child in node.children() {
        if let Some(el) = scraper::ElementRef::wrap(child) {
            collect_text(el, skip, out);
        } else if let Some(text) = child.value().as_text() {
            out.push_str(text);
        }
    }
    if block && !out.ends_with('\n') {
        out.push('\n');
    }
}

#[tool(
    description = "Fetch a web page and extract its main text content. Returns {url, title, text, truncated, cached}. Use after `search` to read the full content of a promising result. Text is cleaned of scripts/styles/navigation and collapsed; truncated to `max_chars` (default 4000, max 12000) to protect token budgets. Results are cached for 10 minutes. Read-only: nothing is mutated.",
    read_only_hint = true,
    destructive_hint = false,
    idempotent_hint = true,
    open_world_hint = true
)]
pub async fn extract(
    url: String,
    max_chars: Option<usize>,
) -> Result<ExtractOutcome, ToolError> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err(ArgosError::InvalidQuery("url must not be empty".into()).into());
    }
    // Basic URL validation
    if url::Url::parse(&url).is_err() {
        return Err(ArgosError::InvalidQuery(format!("invalid url: {url}")).into());
    }

    let budget = max_chars
        .unwrap_or(DEFAULT_EXTRACT_CHARS)
        .clamp(100, MAX_EXTRACT_CHARS);

    // Cache hit
    if let Some((title, text)) = cache::get(&url) {
        let truncated = text.len() > budget;
        let text = if truncated {
            flojo_mcp::truncate_string(&text, budget).0
        } else {
            text
        };
        return Ok(ExtractOutcome {
            url,
            title,
            text,
            truncated,
            cached: true,
        });
    }

    let config = Config::from_env();
    let client = crate::providers::impersonated_client(&config);
    let response = client
        .get(&url)
        .timeout(config.request_timeout)
        .send()
        .await
        .map_err(|ex| ArgosError::Unreachable {
            origin: url.clone(),
            cause: ex.to_string(),
        })?;

    let status = response.status();
    if !status.is_success() {
        let code = status.as_u16();
        return Err(ArgosError::Unreachable {
            origin: url,
            cause: format!("HTTP {code}"),
        }
        .into());
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let body = response
        .text()
        .await
        .map_err(|ex| ArgosError::Unreachable {
            origin: url.clone(),
            cause: ex.to_string(),
        })?;

    let (title, text) = if content_type.contains("text/html") || body.trim_start().starts_with('<') {
        extract_text(&body)
    } else {
        // Plain text / JSON: return as-is (truncated later).
        (String::new(), body)
    };

    cache::put(&url, title.clone(), text.clone());

    let truncated = text.len() > budget;
    let text = if truncated {
        flojo_mcp::truncate_string(&text, budget).0
    } else {
        text
    };

    Ok(ExtractOutcome {
        url,
        title,
        text,
        truncated,
        cached: false,
    })
}
