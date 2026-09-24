//! Centralized configuration (env-driven, testable).

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Runtime configuration for Argos Engine.
#[derive(Debug, Clone)]
pub struct Config {
    /// Base URL of the local SearXNG instance.
    pub searxng_url: String,
    /// Timeout for search requests.
    pub request_timeout: Duration,
    /// Timeout for health probes.
    pub ping_timeout: Duration,
    /// Boot the WSL2 stack on demand when the instance is down (`ARGOS_AUTO_START`).
    pub auto_start: bool,
    /// Budget for a cold stack boot (`ARGOS_BOOT_TIMEOUT_SECS`).
    pub boot_timeout: Duration,
    /// Terminate the stack after this much idle time (`ARGOS_IDLE_STOP_SECS`, `0` = keep).
    pub idle_stop: Duration,
    /// WSL distro hosting the stack (`ARGOS_WSL_DISTRO`).
    pub wsl_distro: String,
    /// Path to `wsl-setup.sh` (`ARGOS_STACK_SCRIPT`), default resolved from the exe location.
    pub stack_script: PathBuf,
    /// Enabled providers in fanout order (`ARGOS_PROVIDERS`, comma-separated).
    pub providers: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            searxng_url: "http://127.0.0.1:8080".into(),
            request_timeout: Duration::from_secs(15),
            ping_timeout: Duration::from_secs(2),
            auto_start: true,
            boot_timeout: Duration::from_secs(60),
            idle_stop: Duration::from_secs(300),
            wsl_distro: "Ubuntu".into(),
            stack_script: default_stack_script(),
            providers: vec!["duckduckgo".into(), "bing".into()],
        }
    }
}

/// Resolve `searxng/wsl-setup.sh` relative to the exe (`target/{release,debug}/`).
fn default_stack_script() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        // exe lives at <repo>/target/<profile>/argos-engine.exe -> two levels up is the repo root.
        .map(|dir| dir.join("../..").join("searxng").join("wsl-setup.sh"))
        .unwrap_or_else(|| PathBuf::from("searxng/wsl-setup.sh"))
}

fn parse_bool(raw: &str, default: bool) -> bool {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => default,
    }
}

fn parse_secs(name: &str, default: Duration) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .map_or(default, Duration::from_secs)
}

/// Split a comma-separated provider list into normalized lowercase names.
fn parse_providers(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|part| part.trim().to_ascii_lowercase())
        .filter(|part| !part.is_empty())
        .collect()
}

impl Config {
    /// Load configuration, applying environment overrides.
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Ok(url) = std::env::var("ARGOS_SEARXNG_URL") {
            let trimmed = url.trim().trim_end_matches('/').to_string();
            if !trimmed.is_empty() {
                config.searxng_url = trimmed;
            }
        }
        if let Ok(raw) = std::env::var("ARGOS_AUTO_START") {
            config.auto_start = parse_bool(&raw, config.auto_start);
        }
        if let Ok(raw) = std::env::var("ARGOS_WSL_DISTRO") {
            let trimmed = raw.trim().to_string();
            if !trimmed.is_empty() {
                config.wsl_distro = trimmed;
            }
        }
        if let Ok(path) = std::env::var("ARGOS_STACK_SCRIPT") {
            let trimmed = path.trim().to_string();
            if !trimmed.is_empty() {
                config.stack_script = PathBuf::from(trimmed);
            }
        }
        config.boot_timeout = parse_secs("ARGOS_BOOT_TIMEOUT_SECS", config.boot_timeout);
        config.idle_stop = parse_secs("ARGOS_IDLE_STOP_SECS", config.idle_stop);
        if let Ok(raw) = std::env::var("ARGOS_PROVIDERS") {
            let providers = parse_providers(&raw);
            if !providers.is_empty() {
                config.providers = providers;
            }
        }
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_points_to_localhost_with_lifecycle_enabled() {
        let config = Config::default();
        assert_eq!(config.searxng_url, "http://127.0.0.1:8080");
        assert_eq!(config.request_timeout, Duration::from_secs(15));
        assert_eq!(config.ping_timeout, Duration::from_secs(2));
        assert!(config.auto_start);
        assert_eq!(config.boot_timeout, Duration::from_secs(60));
        assert_eq!(config.idle_stop, Duration::from_secs(300));
        assert_eq!(config.wsl_distro, "Ubuntu");
        assert!(config.stack_script.to_string_lossy().contains("searxng"));
        assert_eq!(config.providers, vec!["duckduckgo", "bing"]);
    }

    #[test]
    fn provider_list_parsing_is_forgiving() {
        assert_eq!(
            parse_providers(" DuckDuckGo , BING ,,  "),
            vec!["duckduckgo", "bing"]
        );
        assert!(parse_providers(" , , ").is_empty());
    }

    #[test]
    fn bool_parsing_is_forgiving() {
        assert!(parse_bool("true", false));
        assert!(parse_bool("YES", false));
        assert!(parse_bool("on", false));
        assert!(!parse_bool("0", true));
        assert!(!parse_bool("False", true));
        assert!(parse_bool("garbage", true));
    }
}
