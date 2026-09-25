# Argos Engine

**Token-efficient, local-first web research MCP server in Rust.** Keyless multi-engine search with parallel fanout, dedup and automatic failover — no API keys, no runtime, **no VM required**. Built on [FlojoMCP](https://github.com/CerebroCanibalus/FlojoMCP) over the official Rust MCP SDK (`rmcp`).

**Keywords**: MCP server, web search, deep research, metasearch, keyless, token efficiency, AI agents, Model Context Protocol, local-first, Rust, DuckDuckGo, Bing.

## Why

Search MCPs force a bad trade: scrape fragile engines yourself, rent an API key, or run a container farm — and either way the agent pays for it in tokens and timeouts.

- **Keyless by default**: DuckDuckGo, Bing and Brave HTML endpoints queried directly from Rust for compatibility. No accounts, no cards, no Docker, no WSL. The direct HTML set is experimental: see the measured quality and policy caveats below before treating it as a durable index.
- **Browser impersonation**: the HTTP layer ([primp](https://github.com/deedy5/primp), same crate ddgs uses) masquerades as Chrome153/Windows — measured difference: without it Bing serves a `302` to its homepage and Brave returns `429`; with it all three engines answer `200`.
- **Fanout with failover**: all providers queried in parallel, results merged round-robin and deduplicated by URL. When one engine rate-limits your IP (202/403/429 happen — see below), **the others still answer**: research degrades, it doesn't die.
- **Tokens are a feature**: compact results, server-side truncation budgets (`src/limits.rs`), dedup before the agent ever sees duplicates.
- **No silent failures**: every `search` response carries the per-provider status (`ok`, `empty`, `filtered`, `rate_limited`, `unreachable`) and the warnings. If all engines were throttled, the tool returns a typed `AllProvidersRateLimited` error; if providers answered but every result failed Argos' domain/relevance gate, it returns `NoUsableResults` instead of pretending the topic had no hits.
- **Honest, typed errors**: `rate_limited`, `unreachable`, `still starting`, `all_providers_rate_limited`, `no_usable_results` — each with an actionable hint, never a generic `Error executing tool` ghost.
- **Per-provider visibility**: every `search` response carries `{ results, providers: [{name, kind}], warnings }` so the agent knows exactly which engines contributed, which were empty or filtered, and which errored.
- **One small binary**: ~7 MB exe, ~5 ms startup, ~4 MB RAM — no Python/Node runtime.
- **Optional SearXNG adapter**: already written for machines with Docker/WSL2 (70+ engines behind one JSON API) — off by default.

## Status

Early development — **Milestone 1: keyless multi-provider search**. Not production-ready yet.

| Feature | Status |
|---|---|
| `search` via keyless fanout (DuckDuckGo + Bing + Brave), dedup + failover | ✅ |
| Browser impersonation (primp / Chrome153) — Bing & Brave pass | ✅ |
| `status` with per-provider reachability probes | ✅ |
| Typed errors with hints (`rate_limited`, `all_rate_limited`, `unreachable`, ...) | ✅ |
| Real-markup fixture tests (captured HTML from live engines) | ✅ |
| Optional SearXNG adapter + on-demand WSL2 stack lifecycle | ✅ (opt-in via `ARGOS_PROVIDERS`) |
| More keyless providers | 📋 provider audit first; see [PROVIDER_AUDIT.md](PROVIDER_AUDIT.md) |
| `research` tool: multi-query fan-out, progress + cancellation, compact digest | 📋 M2 |
| Content extraction (readability → markdown, no XPath) | 📋 M3 |
| Optional cloud providers (Serper/Tavily/Exa) behind `SearchProvider` | 📋 M3+ |

## Requirements

- Rust 1.89+ ([rustup](https://rustup.rs)) — primp's floor
- Windows: VS Build Tools (the `.bat` scripts set up `VsDevCmd`); plain `cargo build` works on any platform.
- **Nothing else** for the default path. Docker/WSL2 only if you opt into the SearXNG adapter.

## Quick start

```bash
cargo build --release    # or build.bat on Windows (release + tests)
```

Connect from `opencode.jsonc`:

```jsonc
"argos": {
  "type": "local",
  "command": ["D:\\Mis Juegos\\ClaudeMCPs\\argos-engine\\target\\release\\argos-engine.exe"]
}
```

Or any MCP client (Claude Desktop, Cursor, Inspector…): command = the `argos-engine` binary, stdio transport.

### Optional: the SearXNG adapter

```bash
# only if you want the70+ engine aggregator behind the same trait
searxng\wsl-setup.bat        # Windows without Docker Desktop (WSL2, one-time, admin)
# or, with a working Docker:
docker compose -f searxng/docker-compose.yml up -d
# then enable it:
set ARGOS_PROVIDERS=duckduckgo,bing,searxng
```

## Tools

| Tool | Arguments | Returns |
|---|---|---|
| `search` | `query`, `limit?` (1-50, default 10), `page?`, `domains?` (`[String]`, e.g. `["kvrforums.com", "reddit.com"]`) | `{ results: [{ url, source, title, snippet, engine }], providers: [{name, kind: "ok"|"empty"|"filtered"|"rate_limited"|"unreachable", ...}], warnings: [...] }` |
| `status` | – | `{ name, version, searxng_url, searxng_reachable, providers: [{name, reachable}] }` |

### Configuration

| Env var | Default | Meaning |
|---|---|---|
| `ARGOS_PROVIDERS` | `duckduckgo,bing,brave` | Enabled providers, fanout order (adds `searxng` to opt in) |
| `ARGOS_SEARXNG_URL` | `http://127.0.0.1:8080` | Adapter base URL |
| `ARGOS_AUTO_START` | `1` | Boot the WSL2 stack on demand when the adapter is enabled and down |
| `ARGOS_IDLE_STOP_SECS` | `300` | Terminate the stack after idle (`0` = keep) |
| `ARGOS_BOOT_TIMEOUT_SECS` | `60` | Cold-boot budget before returning "still starting" |
| `ARGOS_WSL_DISTRO` | `Ubuntu` | WSL distro hosting the adapter stack |
| `ARGOS_STACK_SCRIPT` | auto | Path to `searxng/wsl-setup.sh` |

## The rate-limit reality (measured, not guessed)

Anti-bot limits live in the **upstream engines**, not in your client — and *every* keyless approach shares that wall (SearXNG's own docs admit it gets CAPTCHAs too; its limiter exists to throttle *you*). Measurements from one residential IP,2026-09-24:

| Endpoint | Plain client (reqwest, rustls **and** schannel) | With primp (Chrome153 impersonation) |
|---|---|---|
| `html.duckduckgo.com` | 200 with browser UA / 403 with PowerShell UA | **200**,10-12 organic results |
| `www.bing.com/search` | **302 → homepage** (same headers/HTTP1.1 that curl passes — pure ClientHello discrimination) | **200**,10 `b_algo` |
| `search.brave.com/search` | **429** | **200**,20 web results |
| `www.mojeek.com/search` | CAPTCHA | CAPTCHA (IP/consent-based — out of the set) |

Argos raises the ceiling with design instead of hoping: **browser impersonation at the transport layer**, **parallel fanout across independent rate budgets**, fair merge + URL dedup (fewer redundant queries for the agent), per-request timeouts, typed rate-limit errors with failover, and — in the SearXNG adapter — on-demand lifecycle so nothing idles.

## Quality reality (measured 2026-09-24)

HTTP 200 is not a quality result. A controlled live audit found that DDG and Brave can return highly relevant niche results on their first request and then quickly return 202/429 from the same IP. Bing returned valid HTML with `b_algo` entries but poor semantic coverage for the same queries: `free piano VST` produced free-game pages, and one `Spitfire Audio LABS free` run produced Macao travel pages. The query itself was present in Bing's final URL, so this was not a URL-encoding failure.

Argos therefore applies a conservative result-side quality gate:

- `domains` is validated and enforced against the returned host; `site:` text is only a ranking hint, never the security boundary.
- Results with no meaningful query terms are rejected as obvious entity drift.
- A provider whose entire result set was rejected is `filtered`, not `ok`.
- If no provider contributes a usable result, the tool returns typed `NoUsableResults`.

The full matrix and policy notes are in [PROVIDER_AUDIT.md](PROVIDER_AUDIT.md). The direct HTML endpoints remain a compatibility set, not a guarantee of quality or long-term provider availability.

## Comparison (researched 2026-09-23)

| | **Argos Engine** | ddgs MCP (Python) | Cloud search MCPs (Tavily / Serper / Exa) |
|---|---|---|---|
| Runtime | single ~7 MB exe | Python ≥3.10 + pip package | binary + API key |
| Startup / RAM | ~5 ms / ~4 MB | ~324 ms / ~50-76 MB¹ | n/a |
| Keys & cost | **none** | none (direct scraping) | keys, free tiers then paid |
| Multi-engine failover | ✅ parallel fanout | sequential per-call engines | provider-dependent |
| Deadlines / typed errors | ✅ | ❌ blocking threads, opaque errors | provider-dependent |
| Engine maintenance | sync from ddgs reference + own parsers | per-engine XPath, constant upstream fixes | provider's problem |
| Deep-research fan-out | roadmap (M2) | no — N sequential LLM calls | partial (their own APIs) |

¹ FastMCP/Python figures from [FlojoMCP benchmarks](https://github.com/CerebroCanibalus/FlojoMCP/blob/main/BENCHMARKS.md); ddgs behavior from source inspection of v9.16.0.

## Roadmap

1. **M1** — keyless fanout (DDG + Bing + Brave, primp impersonation), dedup, typed errors, fixtures, per-provider outcome and validated `domains` scoping ← *current*
2. **M1.1** — provider quality gates, result-side scope enforcement and `NoUsableResults`; see [PROVIDER_AUDIT.md](PROVIDER_AUDIT.md) ← *current*
3. **M1.2** — demote or explicitly opt in direct HTML providers based on the audit; add backoff/circuit state
4. **M2** — `research` tool: multi-query fan-out with reformulation, optional source allowlist, progress + cancellation, compact digest (ChatGPT/Qwen deep-research style)
5. **M3** — content extraction (Rust readability, no XPath) and optional structured remote providers behind the same trait

Decisions and progress live in [AGENTS.md](./AGENTS.md).

## License

GPL-3.0 — see [LICENSE](./LICENSE).

## Credits

- [FlojoMCP](https://github.com/CerebroCanibalus/FlojoMCP) — Rust MCP framework this server is built on.
- [ddgs](https://github.com/deedy5/ddgs) — reference parsers and the pain points that shaped this design.
- [SearXNG](https://github.com/searxng/searxng) — the optional metasearch adapter.
