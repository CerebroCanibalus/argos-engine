# Changelog

All notable Argos Engine changes are recorded here.

## 0.4.0 — public API adapters and fusion hardening

- Added keyless public API adapters for the `academic` profile: **OpenAlex**, **Crossref** and **arXiv**, each with a pure fixture-tested parser.
- Added the `profile` argument to `search`: `general` (DuckDuckGo, Bing, Brave, optional SearXNG) and `academic` (OpenAlex, Crossref, arXiv). Profiles are disjoint families, and an unknown profile is rejected before any provider is contacted.
- Replaced the round-robin interleave with weighted RRF, canonical URL dedup (scheme/`www.`/fragment/trailing slash) and a two-results-per-source concentration guard.
- Added provider quality weighting: the audit measured Bing's HTML SERP as semantically poor, so it votes with reduced weight and cannot dominate a fused ranking.
- Added a 60-second process-local successful-search cache plus a per-key single-flight lock, so concurrent identical calls share one upstream request.
- Added `Retry-After` propagation through `ArgosError`, `ProviderStatus` and shared cooldown state. OpenAlex also reads its JSON `retryAfter` field; a server delay now overrides the conservative local cooldown.
- Reduced the default first wave to two providers and raised fallback capacity to five, keeping noisy engines out of the common path.
- Added `ARGOS_CONTACT_EMAIL` so OpenAlex and Crossref requests can enter their polite pools. It is a contact address, not a key.
- Fixed two live-API defects found by smoke testing: Crossref's `/works` rejects the `page` parameter with HTTP 400 (pagination is now `offset`-based), and OpenAlex sends explicit JSON `null` for sparse records, which previously aborted the entire response.

## 0.3.0 — native metasearch routing

- Added the M1.2a native provider registry and selector.
- Added process-shared provider runtime state, cooldown after rate limits, unreachable backoff, `NoEligibleProviders`, and bounded fallback waves.
- Added `ARGOS_META_INITIAL` and `ARGOS_META_TOTAL` controls.
- Kept the current four implemented providers compatible; public/keyless adapters are the next M1.3 wave.

## Unreleased

- Planned: Semantic Scholar, Europe PMC, PubMed, DOAJ and GDELT adapters, then Wikimedia, Mwmbl, Wiby and the npm/crates.io/Packagist registry family.

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
