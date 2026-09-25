# Changelog

All notable Argos Engine changes are recorded here.

## 0.3.0 — native metasearch routing

- Added the M1.2a native provider registry and selector.
- Added process-shared provider runtime state, cooldown after rate limits, unreachable backoff, `NoEligibleProviders`, and bounded fallback waves.
- Added `ARGOS_META_INITIAL` and `ARGOS_META_TOTAL` controls.
- Kept the current four implemented providers compatible; public/keyless adapters are the next M1.3 wave.

## Unreleased

- Planned M1.2 Native Metasearch Engine: provider registry, profiles, index-family diversity, circuit breaker, quota/cooldown state, RRF fusion and compact token-aware output.
- Added `PROVIDERS.md` as the categorized source of truth for 30 candidate adapters, prioritizing keyless/public APIs before user-key free tiers, paid providers and experimental HTML endpoints.
- Planned M1.2 continuation: RRF fusion, Retry-After propagation, quota ledger and public API adapters.

## 0.2.1 — provider quality gate

- Audited DuckDuckGo, Bing and Brave with a reproducible live matrix; findings are recorded in [PROVIDER_AUDIT.md](PROVIDER_AUDIT.md).
- Added validated `domains` allowlists and result-side host enforcement.
- Added a conservative query relevance gate that rejects obvious entity drift.
- Added `ProviderStatus::Filtered` and typed `NoUsableResults` so unusable upstream HTML cannot masquerade as a successful empty search.
- Corrected the MCP server identity from 0.1.0 to 0.2.0.

## 0.2.0 — visible provider outcomes

- Changed `search` to return `{results, providers, warnings}`.
- Added per-provider `ok`, `empty`, `rate_limited` and `unreachable` states.
- Added `AllProvidersRateLimited` for a total anti-bot failure.
- Added compact `source` hostnames and `search.domains` site-scoping hints.

## 0.1.0 — keyless multi-provider baseline

- Added DuckDuckGo, Bing and Brave direct providers with primp browser impersonation.
- Added parallel fanout, URL deduplication and optional SearXNG adapter.
