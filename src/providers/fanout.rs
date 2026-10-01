//! Parallel fanout across providers: fair merge, URL dedup, full visibility.
//!
//! The fanout never silently hides a provider failure: every configured
//! engine is reported in [`SearchOutcome::providers`], with a typed
//! [`ProviderStatus`], and any degradation surfaces in [`SearchOutcome::warnings`].
//! When every provider rate-limited, the search is upgraded to
//! [`ArgosError::AllProvidersRateLimited`] so the agent knows the empty
//! result came from anti-bot pressure, not from a genuinely empty topic.

use std::collections::{HashMap, HashSet};

use futures::future::join_all;

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::SearchProvider;
use crate::providers::arxiv::ArxivProvider;
use crate::providers::bing::BingProvider;
use crate::providers::brave::BraveProvider;
use crate::providers::crossref::CrossrefProvider;
use crate::providers::duckduckgo::DuckDuckGoProvider;
use crate::providers::openalex::OpenAlexProvider;
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
                "openalex" => providers.push((
                    name.clone(),
                    Box::new(OpenAlexProvider::new(config.clone())),
                )),
                "crossref" => providers.push((
                    name.clone(),
                    Box::new(CrossrefProvider::new(config.clone())),
                )),
                "arxiv" => {
                    providers.push((name.clone(), Box::new(ArxivProvider::new(config.clone()))))
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
            let all_rate_limited: Vec<(String, u16, Option<u64>)> = errors
                .iter()
                .filter_map(|(name, err)| match err {
                    ArgosError::RateLimited {
                        status,
                        retry_after_secs,
                        ..
                    } => Some((name.clone(), *status, *retry_after_secs)),
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

        let merged = rrf_merge(&per_provider_views, limit);

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
                Some(ArgosError::RateLimited {
                    status,
                    retry_after_secs,
                    ..
                }) => ProviderStatus::RateLimited {
                    status: *status,
                    retry_after_secs: *retry_after_secs,
                },
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

/// RRF (Reciprocal Rank Fusion) candidate accumulator.
struct RankedCandidate {
    result: SearchResult,
    score: f64,
    matches: usize,
}

/// Merge provider rankings with weighted RRF, canonical URL dedup and a domain
/// concentration guard.
///
/// Two changes over a plain round-robin interleave matter under pressure:
/// a URL that several independent indexes agree on outranks a single index's
/// top hit, and provider quality enters the vote. A provider whose results are
/// mostly rejected by the quality gate therefore cannot dominate merely by
/// answering first, which is exactly what Bing's HTML SERP used to do.
fn rrf_merge(views: &[ProviderView<'_>], limit: usize) -> Vec<SearchResult> {
    const RRF_K: f64 = 60.0;
    let mut ranked: HashMap<String, RankedCandidate> = HashMap::new();

    for view in views {
        if view.results.is_empty() {
            continue;
        }
        // Accepted/total ratio, floored so a provider is never fully erased by
        // one stray rejection.
        let quality = if view.returned == 0 {
            0.0
        } else {
            (view.results.len() as f64 / view.returned as f64).clamp(0.2, 1.0)
        };
        let weight = provider_weight(&view.name) * quality;
        for (rank, result) in view.results.iter().enumerate() {
            let key = normalize_url(&result.url);
            let contribution = weight / (RRF_K + rank as f64 + 1.0);
            let candidate = ranked.entry(key).or_insert_with(|| RankedCandidate {
                result: result.clone(),
                score: 0.0,
                matches: 0,
            });
            candidate.score += contribution;
            candidate.matches += 1;
            // Keep the richest metadata when duplicates disagree.
            if result.snippet.chars().count() > candidate.result.snippet.chars().count() {
                candidate.result.snippet = result.snippet.clone();
            }
        }
    }

    let mut ordered: Vec<(String, RankedCandidate)> = ranked.into_iter().collect();
    ordered.sort_by(|(left_key, left), (right_key, right)| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| right.matches.cmp(&left.matches))
            .then_with(|| left_key.cmp(right_key))
    });

    let mut selected = Vec::with_capacity(limit);
    let mut selected_keys: HashSet<String> = HashSet::new();
    let mut source_counts: HashMap<String, usize> = HashMap::new();
    // First pass allows at most two results per source host; the second pass
    // fills the remainder when the corpus is genuinely concentrated.
    for max_per_source in [2usize, usize::MAX] {
        for (key, candidate) in &ordered {
            if selected.len() >= limit {
                return selected;
            }
            if !selected_keys.insert(key.clone()) {
                continue;
            }
            let source = candidate.result.source.clone();
            let count = source_counts.get(&source).copied().unwrap_or(0);
            if count >= max_per_source {
                continue;
            }
            *source_counts.entry(source).or_insert(0) += 1;
            selected.push(candidate.result.clone());
        }
    }
    selected
}

/// Relative trust per provider index, from the 2026-09-24 provider audit.
///
/// Measured on this machine: Brave's independent index was accurate, DDG was
/// mixed, and Bing returned real HTML with poor semantic quality (a query for
/// free piano VSTs surfaced games and unrelated tourist pages).
fn provider_weight(name: &str) -> f64 {
    match name {
        "bing" => 0.45,
        "duckduckgo" | "searxng" => 0.9,
        _ => 1.0,
    }
}

/// Canonical URL key: host, port, path and query survive; scheme, `www.`,
/// fragment and trailing-slash differences must not split a duplicate hit.
fn normalize_url(url: &str) -> String {
    if let Ok(parsed) = url::Url::parse(url) {
        let host = parsed
            .host_str()
            .unwrap_or_default()
            .trim_start_matches("www.");
        let port = parsed
            .port()
            .map(|value| format!(":{value}"))
            .unwrap_or_default();
        let path = parsed.path().trim_end_matches('/');
        let query = parsed
            .query()
            .map(|value| format!("?{value}"))
            .unwrap_or_default();
        return format!("{host}{port}{path}{query}");
    }

    url.trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.")
        .split('#')
        .next()
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string()
}

/// Render the human-readable warning for a degraded provider status.
fn warnings_for(name: &str, status: &ProviderStatus) -> Option<String> {
    match status {
        ProviderStatus::RateLimited {
            status: code,
            retry_after_secs,
        } => Some(match retry_after_secs {
            Some(seconds) => format!("{name}: rate-limited (HTTP {code}); retry after {seconds}s"),
            None => {
                format!("{name}: rate-limited (HTTP {code}); back off or rotate ARGOS_PROVIDERS")
            }
        }),
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
                        retry_after_secs: None,
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
                        retry_after_secs: None,
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
                        retry_after_secs: None,
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
                let names: Vec<&str> = providers.iter().map(|(n, _, _)| n.as_str()).collect();
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
                        retry_after_secs: None,
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
            Some(ProviderStatus::RateLimited {
                status: 429,
                retry_after_secs: None
            })
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
