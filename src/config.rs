//! Centralized configuration (env-driven, testable).

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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            searxng_url: "http://127.0.0.1:8080".into(),
            request_timeout: Duration::from_secs(15),
            ping_timeout: Duration::from_secs(2),
        }
    }
}

impl Config {
    /// Load configuration, applying `ARGOS_SEARXNG_URL` overrides.
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Ok(url) = std::env::var("ARGOS_SEARXNG_URL") {
            let trimmed = url.trim().trim_end_matches('/').to_string();
            if !trimmed.is_empty() {
                config.searxng_url = trimmed;
            }
        }
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_points_to_localhost() {
        let config = Config::default();
        assert_eq!(config.searxng_url, "http://127.0.0.1:8080");
        assert_eq!(config.request_timeout, Duration::from_secs(15));
        assert_eq!(config.ping_timeout, Duration::from_secs(2));
    }
}
