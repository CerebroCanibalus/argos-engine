# Changelog

All notable Argos Engine changes are recorded here.

## Unreleased

- Declared all four MCP tool annotations on both tools (`readOnlyHint`,
  `destructiveHint`, `idempotentHint`, `openWorldHint`). Hosts can now warn the
  user before invoking a tool instead of guessing from the description, and
  directories that require them accept the server.
  `search` is marked read-only even though the optional SearXNG adapter can boot
  a local WSL2 stack on demand: no caller-visible state is mutated, and marking
  every search as mutating would make every host warn on every query.
- Added `tests/e2e_stdio.rs`, which spawns the real binary and speaks NDJSON
  over stdio like an MCP host. It pins the `initialize` version against
  `CARGO_PKG_VERSION`, asserts the four annotations arrive over the wire, checks
  the `status` shape, and proves the `search` argument validation runs before any
  provider is contacted. The handshake previously shipped a hardcoded version
  for two releases; this test is what stops that recurring.
- Bumped `flojo-mcp` to `45eca6c`, which adds annotation support to the `#[tool]`
  macro and the `FlojoTool` trait.

## 0.5.0 - academic wave 2 and the code profile

- Fixed the MCP `initialize` handshake, which advertised a hardcoded `0.3.0`:
  the version literal in the `flojo_mcp` attribute drifted from Cargo.toml for
  two releases while the `status` tool reported the real one. A test now pins
  them together.

New keyless adapters, each with a pure fixture-tested parser:

- europe_pmc: biomedical/life-science index. `resultType=core` is required for
  abstracts; `lite` silently omits them.
- pubmed: E-utilities `esearch` + `esummary`. Prefers the DOI in `elocationid`,
  falling back to the PubMed record when the locator is not a DOI.
- doaj: open-access journals and articles.
- semantic_scholar: citation graph; optional `ARGOS_SEMANTIC_SCHOLAR_KEY`.
- gdelt: global news, with its own `news` profile.
- github: repository search, the anchor of the new `code` profile.
- crates, npm, packagist: the Rust, JavaScript and PHP registries.
- wikimedia: Wikipedia search, in its own `knowledge` profile.

New profiles, each a disjoint provider family:

- `code` (github, crates, npm, packagist) for finding libraries and repositories.
- `news` (gdelt) and `knowledge` (wikimedia).

`status` now takes the same `profile` argument and probes only that family, so
checking health no longer costs requests against sixteen providers.

A quality-gate change the smoke tests forced:

- The lexical relevance filter now applies only to the HTML engines. It exists
  because search pages drift and can answer a different question while returning
  HTTP 200. A curated API is itself the relevance signal, and re-filtering it
  only discarded good results: crates.io was dropping matching crates because
  their canonical URL is the repository rather than the registry page. Domain
  scoping still applies to every provider.

Other changes:

- Added `ARGOS_GITHUB_TOKEN` and `ARGOS_SEMANTIC_SCHOLAR_KEY`. Both are
  optional; keyless remains the default.
- Added a per-upstream pacing gate so GDELT's five-second floor is respected
  locally instead of being paid for with 429s.
- Snippet assembly now dedupes repeated punctuation in the shared funnel. Free
  text from upstreams usually ends in its own full stop, which produced
  "Author, One.. Journal." in every adapter.

Bugs found by smoke testing real payloads:

- Crossref's `/works` rejects the `page` parameter with HTTP 400; pagination is
  now `offset`-based.
- OpenAlex sends explicit JSON `null` for sparse records, which aborted the
  whole response under a strict field type.
- Crossref writes `DOI` and `URL` in upper case; serde is case-sensitive, so the
  parse silently produced zero results.
- PubMed's `esummary` envelope carries a `uids` array beside the documents.
  Serde reads a JSON sequence into a struct without complaint, so that array
  decoded as a paper titled with its first PMID. Only objects are documents now.

## 0.4.0 - public API adapters and fusion hardening## 0.4.0 — public API adapters and fusion hardening

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
