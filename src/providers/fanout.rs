//! Parallel fanout across providers: fair merge, URL dedup, full visibility.
//!
//! The fanout never silently hides a provider failure: every configured
//! engine is reported in [`SearchOutcome::providers`], with a typed
//! [`ProviderStatus`], and any degradation surfaces in [`SearchOutcome::warnings`].
//! When every provider rate-limited, the search is upgraded to
//! [`ArgosError::AllProvidersRateLimited`] so the agent knows the empty
//! result came from anti-bot pressure, not from a genuinely empty topic.

use std::collections::HashSet;

use futures::future::join_all;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::SearchProvider;
use crate::providers::bing::BingProvider;
use crate::providers::brave::BraveProvider;
use crate::providers::duckduckgo::DuckDuckGoProvider;
use crate::providers::searxng::SearxNgProvider;
use crate::quality;
use crate::types::{ProviderEntry, ProviderHealth, ProviderStatus, SearchOutcome, SearchResult};

struct ProviderView<'a> {
    name: String,
    results: Vec<SearchResult>,
    error: Option<&'a ArgosError>,
    returned: usize,
    rejected: usize,
}

/// Fanout over the configured providers: queries all in parallel, merges
/// results round-robin (fair mix), deduplicates by URL and reports the
/// per-provider outcome in the [`SearchOutcome`].
pub struct Fanout {
    providers: Vec<(String, Box<dyn SearchProvider>)>,
}

impl Fanout {
    /// Build the fanout from configuration. Unknown names are ignored (the
    /// `status` tool shows the effective set); an empty result falls back to
    /// the default keyless trio so searches can never silently do nothing.
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

    /// Fan a single query out to every provider and return the merged view.
    #[cfg(test)]
    pub async fn run(
        &self,
        provider_query: &str,
        relevance_query: &str,
        limit: usize,
        page: usize,
        domains: &[String],
    ) -> Result<SearchOutcome, ArgosError> {
        self.run_filtered(None, provider_query, relevance_query, limit, page, domains)
            .await
    }

    /// Fan out only to the IDs selected by the native metasearch router.
    pub async fn run_selected(
        &self,
        selected: &[String],
        provider_query: &str,
        relevance_query: &str,
        limit: usize,
        page: usize,
        domains: &[String],
    ) -> Result<SearchOutcome, ArgosError> {
        let selected_names: HashSet<String> = selected.iter().cloned().collect();
        self.run_filtered(
            Some(&selected_names),
            provider_query,
            relevance_query,
            limit,
            page,
            domains,
        )
        .await
    }

    async fn run_filtered(
        &self,
        selected_names: Option<&HashSet<String>>,
        provider_query: &str,
        relevance_query: &str,
        limit: usize,
        page: usize,
        domains: &[String],
    ) -> Result<SearchOutcome, ArgosError> {
        let active_providers: Vec<&(String, Box<dyn SearchProvider>)> = self
            .providers
            .iter()
            .filter(|(name, _)| {
                selected_names.is_none_or(|selected| selected.contains(name.as_str()))
            })
            .collect();
        let outcomes = join_all(
            active_providers
                .iter()
                .map(|(_, provider)| provider.search(provider_query, limit, page)),
        )
        .await;

        let per_provider: Vec<(String, Result<Vec<SearchResult>, ArgosError>)> = active_providers
            .into_iter()
            .zip(outcomes)
            .map(|((name, _), outcome)| (name.clone(), outcome))
            .collect();

        // All providers failed: dedicated error when every cause is rate-limit,
        // otherwise prefer the most actionable individual error.
        let errors: Vec<(String, ArgosError)> = per_provider
            .iter()
            .filter_map(|(name, outcome)| {
                outcome
                    .as_ref()
                    .err()
                    .cloned()
                    .map(|err| (name.clone(), err))
            })
            .collect();
        if errors.len() == per_provider.len() {
            let all_rate_limited: Vec<(String, u16)> = errors
                .iter()
                .filter_map(|(name, err)| match err {
                    ArgosError::RateLimited { status, .. } => Some((name.clone(), *status)),
                    _ => None,
                })
                .collect();
            if all_rate_limited.len() == errors.len() {
                return Err(ArgosError::AllProvidersRateLimited {
                    providers: all_rate_limited,
                });
            }
            let mut sorted = errors;
            sorted.sort_by_key(|(_, err)| match err {
                ArgosError::RateLimited { .. } => 0,
                _ => 1,
            });
            return Err(sorted
                .into_iter()
                .map(|(_, err)| err)
                .next()
                .unwrap_or_else(|| ArgosError::InvalidQuery("no providers configured".into())));
        }

        // At least one provider answered. Apply Argos' result-side quality
        // gates before merging: an upstream HTML page is not trusted merely
        // because it returned HTTP 200.
        let mut total_returned = 0;
        let mut total_rejected = 0;
        let per_provider_views: Vec<ProviderView<'_>> = per_provider
            .iter()
            .map(|(name, outcome)| match outcome {
                Err(error) => ProviderView {
                    name: name.clone(),
                    results: Vec::new(),
                    error: Some(error),
                    returned: 0,
                    rejected: 0,
                },
                Ok(results) => {
                    let returned = results.len();
                    let mut accepted = Vec::with_capacity(returned);
                    let mut rejected = 0;
                    for result in results {
                        let in_scope = quality::matches_domains(&result.url, domains);
                        let relevant = quality::is_relevant(relevance_query, result);
                        if in_scope && relevant {
                            accepted.push(result.clone());
                        } else {
                            rejected += 1;
                        }
                    }
                    total_returned += returned;
                    total_rejected += rejected;
                    ProviderView {
                        name: name.clone(),
                        results: accepted,
                        error: None,
                        returned,
                        rejected,
                    }
                }
            })
            .collect();

        let mut merged: Vec<SearchResult> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let depth = per_provider_views
            .iter()
            .map(|view| view.results.len())
            .max()
            .unwrap_or(0);
        'merge: for rank in 0..depth {
            for view in &per_provider_views {
                if let Some(result) = view.results.get(rank)
                    && seen.insert(normalize_url(&result.url))
                {
                    merged.push(result.clone());
                    if merged.len() >= limit {
                        break 'merge;
                    }
                }
            }
        }

        let mut providers = Vec::new();
        let mut warnings = Vec::new();
        for ProviderView {
            name,
            results,
            error,
            returned,
            rejected,
        } in per_provider_views
        {
            let status = match error {
                Some(ArgosError::RateLimited { status, .. }) => {
                    ProviderStatus::RateLimited { status: *status }
                }
                Some(ArgosError::Unreachable { origin, cause }) => ProviderStatus::Unreachable {
                    message: format!("{origin}: {cause}"),
                },
                Some(other) => ProviderStatus::Unreachable {
                    message: other.to_string(),
                },
                None if results.is_empty() && rejected == 0 => ProviderStatus::Empty,
                None if results.is_empty() => ProviderStatus::Filtered { returned },
                None => ProviderStatus::Ok {
                    count: results.len(),
                },
            };
            if let Some(warning) = warnings_for(&name, &status) {
                warnings.push(warning);
            }
            if rejected > 0 && !matches!(status, ProviderStatus::Filtered { .. }) {
                warnings.push(format!(
                    "{name}: filtered {rejected} result(s) outside the requested domain or query terms"
                ));
            }
            providers.push(ProviderEntry { name, status });
        }

        if merged.is_empty() {
            return Err(ArgosError::NoUsableResults {
                returned: total_returned,
                rejected: total_rejected,
            });
        }

        Ok(SearchOutcome {
            results: merged,
            providers,
            warnings,
        })
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

/// Render the human-readable warning for a degraded provider status.
fn warnings_for(name: &str, status: &ProviderStatus) -> Option<String> {
    match status {
        ProviderStatus::RateLimited { status: code } => Some(format!(
            "{name}: rate-limited (HTTP {code}); back off or rotate ARGOS_PROVIDERS"
        )),
        ProviderStatus::Filtered { returned } => Some(format!(
            "{name}: returned {returned} result(s), but none passed Argos' domain/relevance gate"
        )),
        ProviderStatus::Unreachable { message } => Some(format!("{name}: unreachable ({message})")),
        ProviderStatus::Ok { .. } | ProviderStatus::Empty => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flojo_mcp::async_trait::async_trait;

    struct Stub {
        results: Vec<SearchResult>,
        error: Option<ArgosError>,
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

    fn result(url: &str, title: &str, engine: &str) -> SearchResult {
        SearchResult {
            source: crate::providers::compact_source(url),
            url: url.into(),
            title: title.into(),
            snippet: "s".into(),
            engine: Some(engine.into()),
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

    fn status_of<'a>(outcome: &'a SearchOutcome, name: &str) -> Option<&'a ProviderStatus> {
        outcome
            .providers
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| &entry.status)
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
        let outcome = fanout.run("q", "q", 10, 1, &[]).await.expect("ok");
        // Dedup normalizes scheme/www/trailing-slash, so the www-variant dup dies:
        assert_eq!(outcome.results.len(), 3, "normalized dup must be dropped");
        assert_eq!(
            outcome
                .results
                .iter()
                .map(|r| r.title.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "two", "three"],
            "fair interleave: rank0 of a, rank1 of a..., dup removed"
        );
        assert!(matches!(
            status_of(&outcome, "a"),
            Some(ProviderStatus::Ok { count: 2 })
        ));
        assert!(matches!(
            status_of(&outcome, "b"),
            Some(ProviderStatus::Ok { count: 2 })
        ));
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
        let outcome = fanout.run("q", "q", 2, 1, &[]).await.expect("ok");
        assert_eq!(outcome.results.len(), 2);
    }

    #[tokio::test]
    async fn prefers_rate_limit_error_when_all_failed_with_mixed_causes() {
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
        let error = fanout
            .run("q", "q", 5, 1, &[])
            .await
            .expect_err("must fail");
        assert!(
            matches!(error, ArgosError::RateLimited { .. }),
            "rate-limit explains the failure better than a timeout, got: {error}"
        );
        assert!(error.to_string().contains("rate-limited"));
    }

    #[tokio::test]
    async fn all_rate_limited_promotes_to_dedicated_error() {
        let fanout = fanout(vec![
            (
                "ddg",
                Stub {
                    results: vec![],
                    error: Some(ArgosError::RateLimited {
                        engine: "ddg".into(),
                        status: 202,
                    }),
                },
            ),
            (
                "bing",
                Stub {
                    results: vec![],
                    error: Some(ArgosError::RateLimited {
                        engine: "bing".into(),
                        status: 429,
                    }),
                },
            ),
        ]);
        let error = fanout
            .run("q", "q", 5, 1, &[])
            .await
            .expect_err("must fail");
        match error {
            ArgosError::AllProvidersRateLimited { providers } => {
                assert_eq!(providers.len(), 2);
                let names: Vec<&str> = providers.iter().map(|(n, _)| n.as_str()).collect();
                assert!(names.contains(&"ddg"));
                assert!(names.contains(&"bing"));
            }
            other => panic!("expected AllProvidersRateLimited, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn partial_failure_surfaces_in_providers_and_warnings() {
        let fanout = fanout(vec![
            (
                "ddg",
                Stub {
                    results: vec![result("https://ok.test/x", "ok", "ddg")],
                    error: None,
                },
            ),
            (
                "brave",
                Stub {
                    results: vec![],
                    error: Some(ArgosError::RateLimited {
                        engine: "brave".into(),
                        status: 429,
                    }),
                },
            ),
        ]);
        let outcome = fanout.run("q", "q", 10, 1, &[]).await.expect("ok");
        assert_eq!(outcome.results.len(), 1);
        assert!(matches!(
            status_of(&outcome, "ddg"),
            Some(ProviderStatus::Ok { count: 1 })
        ));
        assert!(matches!(
            status_of(&outcome, "brave"),
            Some(ProviderStatus::RateLimited { status: 429 })
        ));
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("brave"));
    }

    #[tokio::test]
    async fn filters_results_outside_requested_domains() {
        let fanout = fanout(vec![(
            "bing",
            Stub {
                results: vec![
                    result("https://reddit.com/r/piano", "Piano VST", "bing"),
                    result("https://youtube.com/watch?v=1", "Piano VST", "bing"),
                ],
                error: None,
            },
        )]);
        let outcome = fanout
            .run(
                "piano VST (site:reddit.com)",
                "piano VST",
                10,
                1,
                &["reddit.com".into()],
            )
            .await
            .expect("usable result");
        assert_eq!(outcome.results.len(), 1);
        assert!(matches!(
            status_of(&outcome, "bing"),
            Some(ProviderStatus::Ok { count: 1 })
        ));
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning.contains("filtered 1"))
        );
    }

    #[tokio::test]
    async fn rejects_entity_drift_instead_of_returning_garbage() {
        let fanout = fanout(vec![(
            "bing",
            Stub {
                results: vec![result(
                    "https://tripadvisor.com/Macao",
                    "Macao travel guide",
                    "bing",
                )],
                error: None,
            },
        )]);
        let error = fanout
            .run(
                "Spitfire Audio LABS free",
                "Spitfire Audio LABS free",
                10,
                1,
                &[],
            )
            .await
            .expect_err("entity drift must not be returned");
        assert!(matches!(error, ArgosError::NoUsableResults { .. }));
    }

    #[tokio::test]
    async fn empty_from_all_succeeds_is_not_reported_as_success() {
        let fanout = fanout(vec![
            (
                "ddg",
                Stub {
                    results: vec![],
                    error: None,
                },
            ),
            (
                "bing",
                Stub {
                    results: vec![],
                    error: None,
                },
            ),
        ]);
        let error = fanout
            .run("q", "q", 10, 1, &[])
            .await
            .expect_err("empty upstream set must be typed");
        assert!(matches!(
            error,
            ArgosError::NoUsableResults {
                returned: 0,
                rejected: 0
            }
        ));
    }
}
