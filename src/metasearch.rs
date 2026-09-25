//! Native metasearch registry, runtime state and provider selection.
//!
//! The current providers remain behind [`Fanout`]. This module adds the
//! policy layer needed before adding more adapters: manifests describe what a
//! provider is, shared state remembers cooldowns, and the selector chooses a
//! bounded set of eligible providers instead of launching every engine.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::error::ArgosError;
use crate::providers::fanout::Fanout;
use crate::types::{ProviderEntry, ProviderStatus, SearchOutcome};

/// Runtime state shared by router instances within one process.
pub type SharedProviderStates = Arc<Mutex<HashMap<String, ProviderState>>>;

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
    pub fn record_rate_limited_at(&mut self, now: Instant, status: u16) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.status = ProviderRuntimeStatus::Cooldown;
        self.cooldown_until = Some(now + RATE_LIMIT_COOLDOWN);
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
    /// Build a registry using the process-wide state shared by MCP calls.
    pub fn from_config(config: &Config) -> Self {
        Self::from_config_with_state(config, shared_states())
    }

    /// Build a registry with injected state, useful for deterministic tests.
    pub fn from_config_with_state(config: &Config, states: SharedProviderStates) -> Self {
        let mut manifests = HashMap::new();
        let mut members = Vec::new();
        for id in &config.providers {
            let Some(manifest) = builtin_manifest(id) else {
                continue;
            };
            if manifests.insert(id.clone(), manifest).is_none() {
                members.push(id.clone());
                states
                    .lock()
                    .expect("provider state lock")
                    .entry(id.clone())
                    .or_insert_with(ProviderState::new);
            }
        }
        Self {
            manifests,
            states,
            profile: ProviderProfile {
                id: "general".into(),
                members,
                max_initial: config.metasearch_initial,
                max_total: config.metasearch_total,
            },
        }
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
    pub fn record_rate_limits(&self, providers: &[(String, u16)]) {
        let now = Instant::now();
        let mut states = self.states.lock().expect("provider state lock");
        for (id, status) in providers {
            if let Some(state) = states.get_mut(id) {
                state.record_rate_limited_at(now, *status);
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
                ProviderStatus::RateLimited { status } => {
                    state.record_rate_limited_at(now, *status);
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
}

impl MetasearchRouter {
    pub fn from_config(config: &Config) -> Self {
        Self {
            registry: ProviderRegistry::from_config(config),
            fanout: Fanout::from_config(config),
        }
    }

    pub async fn run(
        &self,
        provider_query: &str,
        relevance_query: &str,
        limit: usize,
        page: usize,
        domains: &[String],
    ) -> Result<SearchOutcome, ArgosError> {
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
                    return Ok(outcome);
                }
                Err(ArgosError::AllProvidersRateLimited { providers }) => {
                    self.registry.record_rate_limits(&providers);
                    for (provider, status) in &providers {
                        warnings.push(format!(
                            "{provider}: rate-limited in an earlier wave (HTTP {status})"
                        ));
                    }
                    last_error = Some(ArgosError::AllProvidersRateLimited { providers });
                }
                Err(ArgosError::RateLimited { engine, status }) => {
                    self.registry
                        .record_rate_limits(&[(engine.clone(), status)]);
                    warnings.push(format!(
                        "{engine}: rate-limited before fallback (HTTP {status})"
                    ));
                    last_error = Some(ArgosError::RateLimited { engine, status });
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
    fn rate_limited_provider_is_skipped_until_cooldown() {
        let config = Config {
            metasearch_initial: 3,
            ..Config::default()
        };
        let states = state();
        let registry = ProviderRegistry::from_config_with_state(&config, states.clone());
        let now = Instant::now();
        registry.record_rate_limits(&[("brave".into(), 429)]);
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
        registry.record_rate_limits(&[("bing".into(), 429)]);
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
