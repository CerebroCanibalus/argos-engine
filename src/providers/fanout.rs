//! Parallel fanout across providers: fair merge, URL dedup, error preference.

use std::collections::HashSet;

use flojo_mcp::async_trait::async_trait;
use futures::future::join_all;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::SearchProvider;
use crate::providers::bing::BingProvider;
use crate::providers::brave::BraveProvider;
use crate::providers::duckduckgo::DuckDuckGoProvider;
use crate::providers::searxng::SearxNgProvider;
use crate::types::{ProviderHealth, SearchResult};

/// Fanout over the configured providers: queries all in parallel, merges
/// results round-robin (fair mix), deduplicates by URL and fails over -
/// if every provider fails, the most actionable error wins (rate-limit first).
pub struct Fanout {
    providers: Vec<(String, Box<dyn SearchProvider>)>,
}

impl Fanout {
    /// Build the fanout from configuration. Unknown names are ignored (the
    /// `status` tool shows the effective set); an empty result falls back to
    /// the default keyless pair so searches can never silently do nothing.
    pub fn from_config(config: &Config) -> Self {
        let mut providers: Vec<(String, Box<dyn SearchProvider>)> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for name in &config.providers {
            if !seen.insert(name.as_str()) {
                continue;
            }
            match name.as_str() {
                "duckduckgo" => providers.push((
                    name.clone(),
                    Box::new(DuckDuckGoProvider::new(config.clone())),
                )),
                "bing" => {
                    providers.push((name.clone(), Box::new(BingProvider::new(config.clone()))))
                }
                "brave" => {
                    providers.push((name.clone(), Box::new(BraveProvider::new(config.clone()))))
                }
                "searxng" => {
                    providers.push((name.clone(), Box::new(SearxNgProvider::new(config.clone()))))
                }
                _ => {}
            }
        }
        if providers.is_empty() {
            providers.push((
                "duckduckgo".into(),
                Box::new(DuckDuckGoProvider::new(config.clone())),
            ));
            providers.push(("bing".into(), Box::new(BingProvider::new(config.clone()))));
            providers.push(("brave".into(), Box::new(BraveProvider::new(config.clone()))));
        }
        Self { providers }
    }

    /// Reachability of each configured provider, in order.
    pub async fn probes(&self) -> Vec<ProviderHealth> {
        let mut health = Vec::with_capacity(self.providers.len());
        for (name, provider) in &self.providers {
            health.push(ProviderHealth {
                name: name.clone(),
                reachable: provider.health().await,
            });
        }
        health
    }
}

/// Dedup key: scheme, `www.` and trailing slash differences must not split a hit.
fn normalize_url(url: &str) -> String {
    url.trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.")
        .trim_end_matches('/')
        .to_string()
}

#[async_trait]
impl SearchProvider for Fanout {
    async fn search(
        &self,
        query: &str,
        limit: usize,
        page: usize,
    ) -> Result<Vec<SearchResult>, ArgosError> {
        let outcomes = join_all(
            self.providers
                .iter()
                .map(|(_, provider)| provider.search(query, limit, page)),
        )
        .await;

        let mut per_provider: Vec<Vec<SearchResult>> = Vec::new();
        let mut errors: Vec<ArgosError> = Vec::new();
        for outcome in outcomes {
            match outcome {
                Ok(results) => per_provider.push(results),
                Err(error) => errors.push(error),
            }
        }

        if per_provider.is_empty() {
            // Every provider failed: surface the most actionable error first
            // (a rate-limit explains what happened better than a timeout).
            errors.sort_by_key(|error| match error {
                ArgosError::RateLimited { .. } => 0,
                _ => 1,
            });
            return Err(errors
                .into_iter()
                .next()
                .unwrap_or_else(|| ArgosError::InvalidQuery("no providers configured".into())));
        }

        // Fair interleave: rank i of every provider before rank i+1 of any,
        // deduplicating by normalized URL, until the limit is reached.
        let mut merged: Vec<SearchResult> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let depth = per_provider.iter().map(Vec::len).max().unwrap_or(0);
        'merge: for rank in 0..depth {
            for results in &per_provider {
                if let Some(result) = results.get(rank)
                    && seen.insert(normalize_url(&result.url))
                {
                    merged.push(result.clone());
                    if merged.len() >= limit {
                        break 'merge;
                    }
                }
            }
        }
        Ok(merged)
    }

    async fn health(&self) -> bool {
        for (_, provider) in &self.providers {
            if provider.health().await {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Stub {
        results: Vec<SearchResult>,
        error: Option<ArgosError>,
    }

    fn result(url: &str, title: &str, engine: &str) -> SearchResult {
        SearchResult {
            url: url.into(),
            title: title.into(),
            snippet: "s".into(),
            engine: Some(engine.into()),
        }
    }

    #[async_trait]
    impl SearchProvider for Stub {
        async fn search(
            &self,
            _query: &str,
            _limit: usize,
            _page: usize,
        ) -> Result<Vec<SearchResult>, ArgosError> {
            match &self.error {
                Some(error) => Err(error.clone()),
                None => Ok(self.results.clone()),
            }
        }

        async fn health(&self) -> bool {
            self.error.is_none()
        }
    }

    fn fanout(stubs: Vec<(&str, Stub)>) -> Fanout {
        Fanout {
            providers: stubs
                .into_iter()
                .map(|(name, stub)| (name.to_string(), Box::new(stub) as Box<dyn SearchProvider>))
                .collect(),
        }
    }

    #[tokio::test]
    async fn interleaves_and_dedups_across_providers() {
        let fanout = fanout(vec![
            (
                "a",
                Stub {
                    results: vec![
                        result("https://www.example.com/", "one", "a"),
                        result("https://other.test/x", "two", "a"),
                    ],
                    error: None,
                },
            ),
            (
                "b",
                Stub {
                    results: vec![
                        // Same page as provider a's first hit (www/scheme/trailing slash).
                        result("http://example.com", "one-dup", "b"),
                        result("https://third.test", "three", "b"),
                    ],
                    error: None,
                },
            ),
        ]);

        let merged = fanout.search("q", 10, 1).await.expect("ok");
        // Dedup normalizes scheme/www/trailing-slash, so the www-variant dup dies:
        assert_eq!(merged.len(), 3, "normalized dup must be dropped");
        assert_eq!(
            merged.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            vec!["one", "two", "three"],
            "fair interleave: rank0 of a, rank1 of a..., dup removed"
        );
    }

    #[tokio::test]
    async fn respects_limit() {
        let fanout = fanout(vec![
            (
                "a",
                Stub {
                    results: vec![
                        result("https://a.test/1", "1", "a"),
                        result("https://a.test/2", "2", "a"),
                        result("https://a.test/3", "3", "a"),
                    ],
                    error: None,
                },
            ),
            (
                "b",
                Stub {
                    results: vec![result("https://b.test/1", "b1", "b")],
                    error: None,
                },
            ),
        ]);
        let merged = fanout.search("q", 2, 1).await.expect("ok");
        assert_eq!(merged.len(), 2);
    }

    #[tokio::test]
    async fn all_failed_prefers_rate_limit_error() {
        let fanout = fanout(vec![
            (
                "timeouty",
                Stub {
                    results: vec![],
                    error: Some(ArgosError::Unreachable {
                        origin: "engine-a".into(),
                        cause: "timed out".into(),
                    }),
                },
            ),
            (
                "limited",
                Stub {
                    results: vec![],
                    error: Some(ArgosError::RateLimited {
                        engine: "engine-b".into(),
                        status: 202,
                    }),
                },
            ),
        ]);
        let error = fanout.search("q", 5, 1).await.expect_err("must fail");
        assert!(
            matches!(error, ArgosError::RateLimited { .. }),
            "rate-limit explains the failure better than a timeout, got: {error}"
        );
        assert!(error.to_string().contains("rate-limited"));
    }
}
