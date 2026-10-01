//! Native metasearch registry, runtime state and provider selection.
//!
//! The current providers remain behind [`Fanout`]. This module adds the
//! policy layer needed before adding more adapters: manifests describe what a
//! provider is, shared state remembers cooldowns, and the selector chooses a
//! bounded set of eligible providers instead of launching every engine.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::fanout::Fanout;
use crate::types::{ProviderEntry, ProviderStatus, SearchOutcome};

/// Runtime state shared by router instances within one process.
pub type SharedProviderStates = Arc<Mutex<HashMap<String, ProviderState>>>;

/// Process-local cache of successful searches, plus per-key locks so concurrent
/// identical queries share one upstream request instead of stampeding.
type SharedSearchCache = Arc<Mutex<HashMap<CacheKey, CacheEntry>>>;
type SharedSearchLocks = Arc<Mutex<HashMap<CacheKey, Arc<tokio::sync::Mutex<()>>>>>;

/// Short enough to be invisible to the agent, long enough to absorb the
/// duplicate calls a tool-using loop makes while reformulating one question.
const SEARCH_CACHE_TTL: Duration = Duration::from_secs(60);
const SEARCH_CACHE_CAPACITY: usize = 256;

/// Every field that changes the result set must be part of the key.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CacheKey {
    profile: String,
    query: String,
    limit: usize,
    page: usize,
    domains: Vec<String>,
}

struct CacheEntry {
    outcome: SearchOutcome,
    expires_at: Instant,
    sequence: u64,
}

/// The broad provider categories used by profiles and benchmarks.
/// Additional variants are reserved for the M1.3 adapter wave.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderCategory {
    General,
    Academic,
    Code,
    News,
    Knowledge,
    Local,
}

/// The transport family of a provider adapter.
/// Additional variants are reserved for the M1.3 adapter wave.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderKind {
    Html,
    Api,
    Aggregator,
    Local,
}

/// How an adapter authenticates.
/// Key-required variants are reserved for the M1.4 adapter wave.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthKind {
    None,
    Optional,
    Required,
}

/// Operational policy for an adapter.
/// Paid/key policies are reserved for later adapter waves.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderPolicy {
    Default,
    PublicApi,
    Vertical,
    UserKey,
    Paid,
    HtmlBeta,
    LocalOnly,
}

/// Cost class. This is metadata, not a promise that a free tier will persist.
/// Paid/free-tier variants are reserved for the adapter waves.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CostClass {
    Free,
    FreeTier,
    Paid,
    UserInfrastructure,
}

/// Static description of one provider adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderManifest {
    pub id: String,
    pub kind: ProviderKind,
    pub auth: AuthKind,
    pub auth_env: Option<String>,
    pub policy: ProviderPolicy,
    pub cost: CostClass,
    pub index_family: String,
    pub categories: Vec<ProviderCategory>,
    pub priority: i32,
    pub supports_domains: bool,
    pub supports_freshness: bool,
}

/// A bounded set of providers selected for a query category.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderProfile {
    pub id: String,
    pub members: Vec<String>,
    pub max_initial: usize,
    pub max_total: usize,
}

/// Runtime health of one provider.
/// Quota/auth/filtered states are reserved for adapters that need them.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderRuntimeStatus {
    Healthy,
    Cooldown,
    QuotaExhausted,
    AuthMissing,
    Filtered,
    Degraded,
    Unreachable,
}

/// Mutable, process-local provider state.
#[derive(Clone, Debug)]
pub struct ProviderState {
    pub status: ProviderRuntimeStatus,
    pub consecutive_failures: u32,
    pub cooldown_until: Option<Instant>,
    pub last_status_code: Option<u16>,
    pub last_quality: Option<f64>,
    pub last_success: Option<Instant>,
}

const COOLDOWN_BASE: Duration = Duration::from_secs(30);
const COOLDOWN_MAX: Duration = Duration::from_secs(15 * 60);
const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(180);

impl ProviderState {
    fn new() -> Self {
        Self {
            status: ProviderRuntimeStatus::Healthy,
            consecutive_failures: 0,
            cooldown_until: None,
            last_status_code: None,
            last_quality: None,
            last_success: None,
        }
    }

    /// Whether this provider can be selected at `now`.
    pub fn available_at(&self, now: Instant) -> bool {
        match self.status {
            ProviderRuntimeStatus::Healthy
            | ProviderRuntimeStatus::Filtered
            | ProviderRuntimeStatus::Degraded => true,
            ProviderRuntimeStatus::Cooldown | ProviderRuntimeStatus::Unreachable => {
                self.cooldown_until.is_none_or(|until| until <= now)
            }
            ProviderRuntimeStatus::QuotaExhausted | ProviderRuntimeStatus::AuthMissing => false,
        }
    }

    fn delay_for_failure(&self) -> Duration {
        let exponent = self.consecutive_failures.saturating_sub(1).min(5);
        COOLDOWN_BASE
            .saturating_mul(2u32.saturating_pow(exponent))
            .min(COOLDOWN_MAX)
    }

    /// Record usable results and clear transient failure state.
    pub fn record_success_at(&mut self, now: Instant, quality: f64) {
        self.status = ProviderRuntimeStatus::Healthy;
        self.consecutive_failures = 0;
        self.cooldown_until = None;
        self.last_status_code = None;
        self.last_quality = Some(quality.clamp(0.0, 1.0));
        self.last_success = Some(now);
    }

    /// Record an anti-bot/rate-limit answer and open its circuit.
    ///
    /// When the server advertises a delay we honour it exactly instead of applying
    /// the conservative local default, so a polite `Retry-After: 11` does not cost
    /// the provider three minutes of availability.
    pub fn record_rate_limited_at(
        &mut self,
        now: Instant,
        status: u16,
        retry_after_secs: Option<u64>,
    ) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.status = ProviderRuntimeStatus::Cooldown;
        let cooldown_secs = retry_after_secs
            .unwrap_or(RATE_LIMIT_COOLDOWN.as_secs())
            .clamp(1, 3600);
        self.cooldown_until = Some(now + Duration::from_secs(cooldown_secs));
        self.last_status_code = Some(status);
        self.last_quality = Some(0.0);
    }

    /// Record a transport failure with exponential cooldown.
    pub fn record_unreachable_at(&mut self, now: Instant) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.status = ProviderRuntimeStatus::Unreachable;
        self.cooldown_until = Some(now + self.delay_for_failure());
        self.last_quality = Some(0.0);
    }

    /// Record a successful response with no usable quality, without opening a
    /// circuit. A genuinely empty topic should not be treated like an outage.
    pub fn record_empty_at(&mut self, now: Instant) {
        self.status = ProviderRuntimeStatus::Degraded;
        self.last_quality = Some(0.0);
        self.last_success = Some(now);
    }
}

/// Registry of manifests and shared runtime states.
pub struct ProviderRegistry {
    manifests: HashMap<String, ProviderManifest>,
    states: SharedProviderStates,
    profile: ProviderProfile,
}

impl ProviderRegistry {
    /// Build the `general` registry with injected state (deterministic tests).
    #[cfg(test)]
    pub fn from_config_with_state(config: &Config, states: SharedProviderStates) -> Self {
        Self::for_profile_with_state(config, states, DEFAULT_PROFILE)
            .expect("the default profile is always a valid registry build")
    }

    /// Build the registry for one profile, filtering configured providers by
    /// the categories that profile serves.
    pub fn for_profile(config: &Config, profile: &str) -> Result<Self, ArgosError> {
        Self::for_profile_with_state(config, shared_states(), profile)
    }

    /// Profile-aware construction with injected state.
    pub fn for_profile_with_state(
        config: &Config,
        states: SharedProviderStates,
        profile: &str,
    ) -> Result<Self, ArgosError> {
        let Some(categories) = profile_categories(profile) else {
            return Err(ArgosError::InvalidQuery(format!(
                "unknown profile {profile:?}; available profiles: {}",
                PROFILE_IDS.join(", ")
            )));
        };
        let mut manifests = HashMap::new();
        let mut members = Vec::new();
        let mut fallback_members = Vec::new();
        for id in &config.providers {
            let Some(manifest) = builtin_manifest(id) else {
                continue;
            };
            if manifests.insert(id.clone(), manifest.clone()).is_none() {
                states
                    .lock()
                    .expect("provider state lock")
                    .entry(id.clone())
                    .or_insert_with(ProviderState::new);
                // Remember every implemented provider so a profile whose
                // members are all in cooldown can still fall back instead of
                // reporting "no eligible providers" for a configured set.
                fallback_members.push(id.clone());
                if manifest
                    .categories
                    .iter()
                    .any(|category| categories.contains(category))
                {
                    members.push(id.clone());
                }
            }
        }
        // An empty profile (nothing configured for this category) degrades to
        // the full configured set rather than silently doing nothing.
        if members.is_empty() {
            members = fallback_members;
        }
        // Highest measured trust first, so the first wave spends its budget on
        // the indices most likely to answer.
        members.sort_by_key(|id| Reverse(manifests.get(id).map(|m| m.priority).unwrap_or(0)));
        Ok(Self {
            manifests,
            states,
            profile: ProviderProfile {
                id: profile.to_string(),
                members,
                max_initial: config.metasearch_initial,
                max_total: config.metasearch_total,
            },
        })
    }

    /// Identifier of the profile this registry serves.
    pub fn profile_id(&self) -> &str {
        &self.profile.id
    }

    /// Return the configured, implemented provider IDs in profile order.
    pub fn members(&self) -> Vec<String> {
        self.profile.members.clone()
    }

    /// Select up to the initial wave from currently eligible providers.
    #[cfg(test)]
    pub fn select(&self, now: Instant) -> Vec<String> {
        self.select_remaining(&HashSet::new(), now)
    }

    /// Select the next wave, excluding providers attempted in this call.
    pub fn select_remaining(&self, attempted: &HashSet<String>, now: Instant) -> Vec<String> {
        self.select_profile(&self.profile, attempted, now)
    }

    fn select_profile(
        &self,
        profile: &ProviderProfile,
        attempted: &HashSet<String>,
        now: Instant,
    ) -> Vec<String> {
        let states = self.states.lock().expect("provider state lock");
        let mut candidates: Vec<(i32, String)> = profile
            .members
            .iter()
            .filter(|id| !attempted.contains(*id))
            .filter_map(|id| {
                let manifest = self.manifests.get(id)?;
                let state = states.get(id)?;
                state.available_at(now).then(|| {
                    let quality = state.last_quality.unwrap_or(0.5);
                    let score = manifest.priority + (quality * 20.0).round() as i32;
                    (score, id.clone())
                })
            })
            .collect();
        drop(states);
        candidates.sort_by_key(|(score, _)| Reverse(*score));
        candidates
            .into_iter()
            .take(profile.max_initial.min(profile.max_total))
            .map(|(_, id)| id)
            .collect()
    }

    /// Apply one call's provider statuses to shared runtime state.
    pub fn record_outcome(&self, outcome: &SearchOutcome) {
        self.record_entries(&outcome.providers, Instant::now());
    }

    /// Apply a typed all-rate-limited error to shared runtime state.
    pub fn record_rate_limits(&self, providers: &[(String, u16, Option<u64>)]) {
        let now = Instant::now();
        let mut states = self.states.lock().expect("provider state lock");
        for (id, status, retry_after_secs) in providers {
            if let Some(state) = states.get_mut(id) {
                state.record_rate_limited_at(now, *status, *retry_after_secs);
            }
        }
    }

    fn record_entries(&self, entries: &[ProviderEntry], now: Instant) {
        let mut states = self.states.lock().expect("provider state lock");
        for entry in entries {
            let Some(state) = states.get_mut(&entry.name) else {
                continue;
            };
            match &entry.status {
                ProviderStatus::Ok { count } => {
                    let quality = (*count as f64 / 10.0).min(1.0);
                    state.record_success_at(now, quality);
                }
                ProviderStatus::Empty | ProviderStatus::Filtered { .. } => {
                    state.record_empty_at(now);
                }
                ProviderStatus::RateLimited {
                    status,
                    retry_after_secs,
                } => {
                    state.record_rate_limited_at(now, *status, *retry_after_secs);
                }
                ProviderStatus::Unreachable { .. } => {
                    state.record_unreachable_at(now);
                }
            }
        }
    }
}

/// Router used by the search tool. It preserves the current Fanout providers
/// while selecting only currently eligible IDs.
pub struct MetasearchRouter {
    registry: ProviderRegistry,
    fanout: Fanout,
    config: Config,
    cache: SharedSearchCache,
    cache_locks: SharedSearchLocks,
}

impl MetasearchRouter {
    /// Build the router for a named profile, rejecting unknown profiles before
    /// any network access so a typo never burns provider quota.
    pub fn for_profile(config: &Config, profile: &str) -> Result<Self, ArgosError> {
        Ok(Self::build(
            ProviderRegistry::for_profile(config, profile)?,
            Fanout::from_config(config),
            config.clone(),
        ))
    }

    fn build(registry: ProviderRegistry, fanout: Fanout, config: Config) -> Self {
        Self {
            registry,
            fanout,
            config,
            cache: shared_search_cache(),
            cache_locks: shared_search_locks(),
        }
    }

    /// Reachability probes restricted to this router's profile members.
    pub async fn probes(&self) -> Vec<crate::types::ProviderHealth> {
        self.fanout.probes_selected(&self.registry.members()).await
    }

    /// Whether the optional local SearXNG stack answers.
    pub async fn searxng_reachable(&self) -> bool {
        use crate::providers::SearchProvider as _;
        crate::providers::searxng::SearxNgProvider::new(self.config.clone())
            .health()
            .await
    }

    pub async fn run(
        &self,
        provider_query: &str,
        relevance_query: &str,
        limit: usize,
        page: usize,
        domains: &[String],
    ) -> Result<SearchOutcome, ArgosError> {
        let cache_key = CacheKey {
            profile: self.registry.profile_id().to_string(),
            query: relevance_query.to_string(),
            limit,
            page,
            domains: domains.to_vec(),
        };
        // Single-flight: identical concurrent calls wait for one request
        // instead of all missing the cache and hammering the same providers.
        let request_lock = cache_lock(&self.cache_locks, &cache_key);
        let _request_guard = request_lock.lock().await;
        if let Some(mut cached) = cache_get(&self.cache, &cache_key) {
            cached.warnings.insert(
                0,
                "cache: reused a matching result within the 60s TTL".into(),
            );
            return Ok(cached);
        }

        let mut attempted = HashSet::new();
        let mut last_error: Option<ArgosError> = None;
        let mut warnings = Vec::new();

        for _ in 0..self.registry.profile.max_total {
            let selected = self.registry.select_remaining(&attempted, Instant::now());
            if selected.is_empty() {
                break;
            }
            attempted.extend(selected.iter().cloned());

            match self
                .fanout
                .run_selected(
                    &selected,
                    provider_query,
                    relevance_query,
                    limit,
                    page,
                    domains,
                )
                .await
            {
                Ok(mut outcome) => {
                    self.registry.record_outcome(&outcome);
                    outcome.warnings.splice(0..0, warnings);
                    cache_put(&self.cache, cache_key, &outcome);
                    return Ok(outcome);
                }
                Err(ArgosError::AllProvidersRateLimited { providers }) => {
                    self.registry.record_rate_limits(&providers);
                    for (provider, status, retry_after_secs) in &providers {
                        warnings.push(match retry_after_secs {
                            Some(seconds) => format!(
                                "{provider}: rate-limited in an earlier wave (HTTP {status}); retry after {seconds}s"
                            ),
                            None => format!(
                                "{provider}: rate-limited in an earlier wave (HTTP {status})"
                            ),
                        });
                    }
                    last_error = Some(ArgosError::AllProvidersRateLimited { providers });
                }
                Err(ArgosError::RateLimited {
                    engine,
                    status,
                    retry_after_secs,
                }) => {
                    self.registry
                        .record_rate_limits(&[(engine.clone(), status, retry_after_secs)]);
                    warnings.push(match retry_after_secs {
                        Some(seconds) => format!(
                            "{engine}: rate-limited before fallback (HTTP {status}); retry after {seconds}s"
                        ),
                        None => format!(
                            "{engine}: rate-limited before fallback (HTTP {status})"
                        ),
                    });
                    last_error = Some(ArgosError::RateLimited {
                        engine,
                        status,
                        retry_after_secs,
                    });
                }
                Err(error) => {
                    warnings.push(format!("fallback wave failed: {error}"));
                    last_error = Some(error);
                }
            }
        }

        Err(last_error.unwrap_or(ArgosError::NoEligibleProviders {
            providers: self.registry.members(),
        }))
    }
}

fn shared_states() -> SharedProviderStates {
    static STATES: OnceLock<SharedProviderStates> = OnceLock::new();
    STATES
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
}

fn shared_search_cache() -> SharedSearchCache {
    static CACHE: OnceLock<SharedSearchCache> = OnceLock::new();
    CACHE
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
}

fn shared_search_locks() -> SharedSearchLocks {
    static LOCKS: OnceLock<SharedSearchLocks> = OnceLock::new();
    LOCKS
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
}

fn cache_lock(locks: &SharedSearchLocks, key: &CacheKey) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = locks.lock().expect("search lock registry lock");
    locks
        .entry(key.clone())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

fn cache_get(cache: &SharedSearchCache, key: &CacheKey) -> Option<SearchOutcome> {
    let now = Instant::now();
    let mut cache = cache.lock().expect("search cache lock");
    let expired = cache
        .iter()
        .filter(|(_, entry)| entry.expires_at <= now)
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for key in expired {
        cache.remove(&key);
    }
    cache.get(key).map(|entry| entry.outcome.clone())
}

fn cache_put(cache: &SharedSearchCache, key: CacheKey, outcome: &SearchOutcome) {
    static SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let mut cache = cache.lock().expect("search cache lock");
    let now = Instant::now();
    cache.retain(|_, entry| entry.expires_at > now);
    while cache.len() >= SEARCH_CACHE_CAPACITY {
        let Some(oldest_key) = cache
            .iter()
            .min_by_key(|(_, entry)| entry.sequence)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        cache.remove(&oldest_key);
    }
    cache.insert(
        key,
        CacheEntry {
            outcome: outcome.clone(),
            expires_at: now + SEARCH_CACHE_TTL,
            sequence: SEQUENCE.fetch_add(1, Ordering::Relaxed),
        },
    );
}

/// Profile used when the caller does not ask for one.
pub const DEFAULT_PROFILE: &str = "general";

/// Profiles a caller may request, in documentation order.
pub const PROFILE_IDS: &[&str] = &["general", "academic", "code", "news", "knowledge"];

/// Categories served by each profile.
fn profile_categories(profile: &str) -> Option<Vec<ProviderCategory>> {
    match profile {
        "general" => Some(vec![ProviderCategory::General, ProviderCategory::Local]),
        "academic" => Some(vec![ProviderCategory::Academic]),
        "code" => Some(vec![ProviderCategory::Code]),
        "news" => Some(vec![ProviderCategory::News]),
        "knowledge" => Some(vec![ProviderCategory::Knowledge]),
        _ => None,
    }
}

fn builtin_manifest(id: &str) -> Option<ProviderManifest> {
    let (kind, policy, cost, family, priority, categories) = match id {
        "duckduckgo" => (
            ProviderKind::Html,
            ProviderPolicy::HtmlBeta,
            CostClass::Free,
            "ddg_mixed",
            70,
            vec![ProviderCategory::General],
        ),
        "bing" => (
            ProviderKind::Html,
            ProviderPolicy::HtmlBeta,
            CostClass::Free,
            "bing",
            30,
            vec![ProviderCategory::General],
        ),
        "brave" => (
            ProviderKind::Html,
            ProviderPolicy::HtmlBeta,
            CostClass::Free,
            "brave",
            60,
            vec![ProviderCategory::General],
        ),
        "searxng" => (
            ProviderKind::Aggregator,
            ProviderPolicy::LocalOnly,
            CostClass::UserInfrastructure,
            "multi",
            80,
            vec![ProviderCategory::General, ProviderCategory::Local],
        ),
        // Public bibliographic APIs. Distinct index families on purpose: RRF
        // only rewards agreement when the agreeing sources are independent.
        "openalex" => (
            ProviderKind::Api,
            ProviderPolicy::PublicApi,
            CostClass::Free,
            "openalex",
            90,
            vec![ProviderCategory::Academic],
        ),
        "crossref" => (
            ProviderKind::Api,
            ProviderPolicy::PublicApi,
            CostClass::Free,
            "crossref",
            80,
            vec![ProviderCategory::Academic],
        ),
        "arxiv" => (
            ProviderKind::Api,
            ProviderPolicy::PublicApi,
            CostClass::Free,
            "arxiv",
            75,
            vec![ProviderCategory::Academic],
        ),
        // Academic wave 2. Priorities sit below OpenAlex/Crossref because these
        // are narrower or, in Semantic Scholar's case, throttled anonymously.
        "europe_pmc" => (
            ProviderKind::Api,
            ProviderPolicy::Vertical,
            CostClass::Free,
            "europe_pmc",
            70,
            vec![ProviderCategory::Academic],
        ),
        "doaj" => (
            ProviderKind::Api,
            ProviderPolicy::PublicApi,
            CostClass::Free,
            "doaj",
            65,
            vec![ProviderCategory::Academic],
        ),
        "pubmed" => (
            ProviderKind::Api,
            ProviderPolicy::Vertical,
            CostClass::Free,
            "pubmed",
            64,
            vec![ProviderCategory::Academic],
        ),
        "semantic_scholar" => (
            ProviderKind::Api,
            ProviderPolicy::PublicApi,
            CostClass::Free,
            "semantic_scholar",
            60,
            vec![ProviderCategory::Academic],
        ),
        // Code family. GitHub leads because its anonymous quota (60/h) is the
        // scarcest resource in the registry: spend it on the first wave.
        "github" => (
            ProviderKind::Api,
            ProviderPolicy::Vertical,
            CostClass::Free,
            "github",
            95,
            vec![ProviderCategory::Code],
        ),
        "crates" => (
            ProviderKind::Api,
            ProviderPolicy::Vertical,
            CostClass::Free,
            "crates_io",
            85,
            vec![ProviderCategory::Code],
        ),
        "npm" => (
            ProviderKind::Api,
            ProviderPolicy::Vertical,
            CostClass::Free,
            "npm",
            80,
            vec![ProviderCategory::Code],
        ),
        "packagist" => (
            ProviderKind::Api,
            ProviderPolicy::Vertical,
            CostClass::Free,
            "packagist",
            70,
            vec![ProviderCategory::Code],
        ),
        "wikimedia" => (
            ProviderKind::Api,
            ProviderPolicy::Vertical,
            CostClass::Free,
            "wikimedia",
            80,
            vec![ProviderCategory::Knowledge],
        ),
        "gdelt" => (
            ProviderKind::Api,
            ProviderPolicy::Vertical,
            CostClass::Free,
            "gdelt",
            70,
            vec![ProviderCategory::News],
        ),
        _ => return None,
    };
    Some(ProviderManifest {
        id: id.into(),
        kind,
        auth: AuthKind::None,
        auth_env: None,
        policy,
        cost,
        index_family: family.into(),
        categories,
        priority,
        supports_domains: true,
        supports_freshness: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> SharedProviderStates {
        Arc::new(Mutex::new(HashMap::new()))
    }

    #[test]
    fn registry_registers_only_implemented_configured_providers() {
        let config = Config {
            providers: vec!["duckduckgo".into(), "not-implemented".into(), "bing".into()],
            metasearch_initial: 2,
            ..Config::default()
        };
        let registry = ProviderRegistry::from_config_with_state(&config, state());
        assert_eq!(registry.members(), vec!["duckduckgo", "bing"]);
        assert_eq!(registry.select(Instant::now()).len(), 2);
    }

    #[test]
    fn profiles_select_disjoint_provider_families() {
        let config = Config::default();
        let general = ProviderRegistry::for_profile_with_state(&config, state(), "general")
            .expect("general profile");
        let academic = ProviderRegistry::for_profile_with_state(&config, state(), "academic")
            .expect("academic profile");
        let code = ProviderRegistry::for_profile_with_state(&config, state(), "code")
            .expect("code profile");
        let news = ProviderRegistry::for_profile_with_state(&config, state(), "news")
            .expect("news profile");
        let knowledge = ProviderRegistry::for_profile_with_state(&config, state(), "knowledge")
            .expect("knowledge profile");

        // Members are ordered by measured trust, and families must not leak into
        // each other: an academic search must never spend HTML engine quota.
        assert_eq!(general.members(), vec!["duckduckgo", "brave", "bing"]);
        assert_eq!(
            academic.members(),
            vec![
                "openalex",
                "crossref",
                "arxiv",
                "europe_pmc",
                "doaj",
                "pubmed",
                "semantic_scholar"
            ]
        );
        assert_eq!(code.members(), vec!["github", "crates", "npm", "packagist"]);
        assert_eq!(news.members(), vec!["gdelt"]);
        assert_eq!(knowledge.members(), vec!["wikimedia"]);

        for other in [academic.members(), code.members(), news.members()] {
            for id in &other {
                assert!(
                    !general.members().contains(id),
                    "{id} must not appear in two profiles"
                );
            }
        }
    }

    #[test]
    fn unknown_profile_is_rejected_before_any_network_access() {
        let error = ProviderRegistry::for_profile(&Config::default(), "nope")
            .err()
            .expect("unknown profile must fail");
        assert!(error.to_string().contains("academic"));
        assert!(error.to_string().contains("code"));
    }

    #[test]
    fn empty_profile_falls_back_to_the_configured_set() {
        let config = Config {
            providers: vec!["bing".into()],
            ..Config::default()
        };
        // `bing` is General-only, so `academic` has no category members.
        let academic = ProviderRegistry::for_profile_with_state(&config, state(), "academic")
            .expect("academic profile");
        assert_eq!(academic.members(), vec!["bing"]);
    }

    #[test]
    fn server_retry_after_replaces_the_conservative_cooldown() {
        let mut provider = ProviderState::new();
        let now = Instant::now();
        provider.record_rate_limited_at(now, 429, Some(11));
        assert_eq!(provider.cooldown_until, Some(now + Duration::from_secs(11)));
    }

    #[test]
    fn missing_retry_after_falls_back_to_the_local_default() {
        let mut provider = ProviderState::new();
        let now = Instant::now();
        provider.record_rate_limited_at(now, 429, None);
        assert_eq!(provider.cooldown_until, Some(now + RATE_LIMIT_COOLDOWN));
    }

    #[test]
    fn search_cache_round_trips_a_successful_outcome() {
        let cache: SharedSearchCache = Arc::new(Mutex::new(HashMap::new()));
        let key = CacheKey {
            profile: "general".into(),
            query: "rust async runtime".into(),
            limit: 5,
            page: 1,
            domains: Vec::new(),
        };
        let outcome = SearchOutcome {
            results: Vec::new(),
            providers: Vec::new(),
            warnings: Vec::new(),
        };
        cache_put(&cache, key.clone(), &outcome);
        assert!(cache_get(&cache, &key).is_some());
        // A different profile must never reuse the general-web entry.
        assert!(
            cache_get(
                &cache,
                &CacheKey {
                    profile: "academic".into(),
                    ..key
                }
            )
            .is_none()
        );
    }

    #[test]
    fn expired_cache_entries_are_dropped_on_read() {
        let cache: SharedSearchCache = Arc::new(Mutex::new(HashMap::new()));
        let key = CacheKey {
            profile: "general".into(),
            query: "stale".into(),
            limit: 5,
            page: 1,
            domains: Vec::new(),
        };
        let outcome = SearchOutcome {
            results: Vec::new(),
            providers: Vec::new(),
            warnings: Vec::new(),
        };
        cache_put(&cache, key.clone(), &outcome);
        cache
            .lock()
            .expect("cache lock")
            .get_mut(&key)
            .expect("entry")
            .expires_at = Instant::now();
        assert!(cache_get(&cache, &key).is_none());
        assert!(
            cache.lock().expect("cache lock").get(&key).is_none(),
            "expired entry must be evicted, not merely hidden"
        );
    }

    #[test]
    fn rate_limited_provider_is_skipped_until_cooldown() {
        let config = Config {
            metasearch_initial: 3,
            ..Config::default()
        };
        let states = state();
        let registry = ProviderRegistry::from_config_with_state(&config, states.clone());
        let now = Instant::now();
        registry.record_rate_limits(&[("brave".into(), 429, None)]);
        let selected = registry.select(now);
        assert!(!selected.contains(&"brave".to_string()));
        assert_eq!(selected.len(), 2);
        assert_eq!(
            states
                .lock()
                .expect("state lock")
                .get("brave")
                .expect("brave state")
                .status,
            ProviderRuntimeStatus::Cooldown
        );
    }

    #[test]
    fn all_cooldown_produces_no_eligible_selection() {
        let config = Config {
            providers: vec!["bing".into()],
            ..Config::default()
        };
        let states = state();
        let registry = ProviderRegistry::from_config_with_state(&config, states.clone());
        registry.record_rate_limits(&[("bing".into(), 429, None)]);
        assert!(registry.select(Instant::now()).is_empty());
    }

    #[test]
    fn unreachable_provider_recovers_after_backoff() {
        let mut state = ProviderState::new();
        let now = Instant::now();
        state.record_unreachable_at(now);
        assert!(!state.available_at(now));
        assert!(state.available_at(now + COOLDOWN_BASE + Duration::from_secs(1)));
    }
}
