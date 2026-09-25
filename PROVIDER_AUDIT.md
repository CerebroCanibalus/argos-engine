# Provider audit — 2026-09-24

This is a protocol and quality audit, not a claim that an HTTP 200 is a useful
search. The measurements were made from the same residential IP used by the
Argos development environment.

## Reproducible live matrix

The harness launches one fresh Argos process per provider and query, with
`ARGOS_PROVIDERS=<provider>`, `limit=10`, and page 1. It records the MCP
outcome, raw result count, provider status, latency, URLs, and warnings.

| Provider | Query | Observed result |
|---|---|---|
| DuckDuckGo | `free Spitfire LABS piano VST`, scoped to KVR/Reddit | 10/10 relevant Reddit results in 774 ms |
| DuckDuckGo | next two queries after 1.5 s spacing | HTTP 202, reported as rate-limited |
| Brave | `free Spitfire LABS piano VST`, scoped to KVR/Reddit | 10/10 relevant Reddit results in 828 ms |
| Brave | next two queries after 1.5 s spacing | HTTP 429, reported as rate-limited |
| Bing | `free piano VST` | Poki, Friv, CrazyGames, Garena, Plex and other free-game/movie pages |
| Bing | `Rust async Tokio tutorial` | Rust language/game pages, Steam and Wikipedia; Tokio is not the dominant intent |
| Bing | `Spitfire Audio LABS free` | on one run: Macao travel pages; on later runs: empty or unrelated SERP results |

The Bing request's final URL retained the complete query. A raw response
inspection showed the HTML was a real Bing SERP with `b_algo` entries, not a
parser failure. Therefore `primp` solved transport blocking, but it did not
solve Bing's ranking/coverage problem on this IP.

## Scope and quality findings

- `domains` previously only appended `(site:...)` to the query. Bing did not
  reliably honor it and returned out-of-scope domains. Argos now validates the
  allowlist and enforces it again on every returned URL.
- A provider returning 200 with an unrelated SERP was previously reported as
  `ok`. Argos now applies a conservative query-term gate and reports rejected
  results. If nothing usable remains, it returns `NoUsableResults` instead of a
  successful empty payload.
- The gate is intentionally not a claim of semantic search. It only rejects
  results with no meaningful query terms (or results outside an explicit
  domain allowlist); ranking quality remains an upstream problem.
- DDG and Brave can be useful when their first request succeeds, but their
  per-IP budget is too small for a burst of independent research queries.
- DDG and Bing share a provider family according to ddgs; Brave has an
  independent crawler/index, but its endpoint budget is also volatile.

## Policy signal

The public `robots.txt` files currently show:

- DuckDuckGo HTML endpoint: `Allow: /`.
- Bing: `Disallow: /search` for the generic user-agent group.
- Brave: `Disallow: /search` for the generic user-agent group.
- Google, Yahoo, Startpage, Mojeek and Yandex also restrict their search
  routes in their public policies.

This is not legal advice, but it is a product-design signal: direct HTML
endpoints are not interchangeable with supported APIs. Argos must not present
Bing/Brave HTML scraping as a durable, compliant quality guarantee. The
remaining decision is whether to demote those providers from the default set
or keep them as explicitly opt-in experimental fallbacks.

## Keyless structured references

A controlled probe of the same niche query against two documented keyless
structured APIs returned relevant results:

- Tavily: 5 results, including ProducerGrind, Reddit, Splice and Spitfire
  Audio, with numeric relevance scores.
- Firecrawl: 5 results, including Spitfire Audio, Reddit, Splice and YouTube.

Both are remote services, not local indexes. They are useful references for
the quality bar and possible optional adapters, but they do not replace a
self-hosted/local index.

## Post-gate verification

The same live harness was rerun against the quality-gated binary:

- Brave returned 10 usable results for the niche VST query, 10 Tokio-focused
  results for the Rust query, and 10 Spitfire/LABS results for the entity
  query.
- Bing returned 10, 10 and 7 raw `b_algo` results respectively, but all were
  rejected by the scope/relevance gate. The MCP response was
  `NoUsableResults` with `returned` and `rejected` counts, never a fabricated
  result list.
- DuckDuckGo returned HTTP 202 for all three isolated calls and the MCP response
  was `AllProvidersRateLimited` with the provider list.

This is the first useful separation between transport availability and research
quality: `reachable` is not the same as `usable`.

## Decision

Do not add more HTML providers until the current three have a quality gate,
scope enforcement, and a repeatable benchmark. The next architectural choice
is explicit:

1. keep a local-first DDG path and make the unreliable direct HTML providers
   opt-in experimental adapters; or
2. add a remote keyless structured provider such as Tavily as a separate
   optional backend; or
3. revive SearXNG when its cost is acceptable, because it can expose many
   independent engines behind one auditable adapter.

The current release keeps the existing provider list for compatibility, but
Argos no longer claims that every HTTP 200 provider contributes usable
research.
