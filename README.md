# Argos Engine

**Token-efficient, local-first web research MCP server in Rust.** One small binary, no API keys, no cloud lock-in — deep web search for AI agents backed by [your own SearXNG](https://github.com/searxng/searxng).

**Keywords**: MCP server, web search, deep research, metasearch, SearXNG, token efficiency, AI agents, Model Context Protocol, local-first, Rust.

## Why

Search MCPs force a bad trade: scrape fragile engines yourself, or rent an API key and bleed credits — and either way the agent pays for it in tokens and timeouts.

- **Tokens**: results come back compact and normalized (truncated titles/snippets, no HTML, no duplicate payloads). The agent reads findings, not boilerplate.
- **Local-first**: a private SearXNG instance aggregates 70+ engines (Google, Bing, DuckDuckGo, Brave, …). No keys, no accounts, no per-query billing. Engine-selector maintenance stays with the SearXNG community — not us.
- **Fast and small**: built on [FlojoMCP](https://github.com/CerebroCanibalus/FlojoMCP) over the official Rust MCP SDK (`rmcp`): a single ~7 MB exe that starts in ~5 ms and idles at ~4 MB — no Python/Node runtime.
- **Honest errors**: typed tool errors with actionable hints (instance down, JSON API disabled, …) instead of opaque `Error executing tool` ghosts.

## Status

Early development — **Milestone 1 (local SearXNG search)**. Not production-ready yet.

| Feature | Status |
|---|---|
| `search` tool via local SearXNG JSON API | ✅ |
| `status` health tool (version + SearXNG probe) | ✅ |
| Typed errors with hints | ✅ |
| Compact, truncated result payloads | ✅ |
| `research` tool: multi-query fan-out, dedup, progress + cancellation, compact digest | 📋 M2 |
| Content extraction (readability → markdown, no XPath) | 📋 M3 |
| Optional cloud providers (Serper/Tavily/Exa/Brave) behind `SearchProvider` | 📋 M3+ |

## Requirements

- Rust 1.85+ ([rustup](https://rustup.rs))
- Docker (runs the local SearXNG): **Docker Desktop** on Windows 19045+/11, any Docker Engine on Linux/macOS — **or**, on Windows builds below 19045 where Desktop refuses to install (e.g. LTSC 21H2), the bundled WSL2 fallback: one-time `searxng\wsl-setup.bat` (admin), then `searxng\wsl-up.bat` per session.
- Windows: VS Build Tools (the `.bat` scripts set up `VsDevCmd`); plain `cargo build` works on any platform.

## Quick start

```bash
# 1. Start local SearXNG with the JSON API enabled (settings included)
docker compose -f searxng/docker-compose.yml up -d

# 2. Verify the JSON API answers
curl "http://127.0.0.1:8080/search?q=test&format=json"

# 3. Build
build.bat          # Windows — release + tests
cargo build --release   # anywhere
```

Connect it from `opencode.jsonc`:

```jsonc
"argos": {
  "type": "local",
  "command": ["D:\\Mis Juegos\\ClaudeMCPs\\argos-engine\\target\\release\\argos-engine.exe"]
}
```

Or any MCP client (Claude Desktop, Cursor, Inspector…): command = the `argos-engine` binary, stdio transport.

## Tools

| Tool | Arguments | Returns |
|---|---|---|
| `status` | – | `{ name, version, searxng_url, searxng_reachable }` |
| `search` | `query`, `limit?` (1-50, default 10), `page?` | `[{ url, title, snippet, engine }]` |

Configuration: env `ARGOS_SEARXNG_URL` (default `http://127.0.0.1:8080`).

## Comparison (researched 2026-09-23)

| | **Argos Engine** | ddgs MCP (Python) | Cloud search MCPs (Tavily / Serper / Exa) |
|---|---|---|---|
| Runtime | single ~7 MB exe | Python ≥3.10 + pip package | binary + API key |
| Startup / RAM | ~5 ms / ~4 MB | ~324 ms / ~50-76 MB¹ | n/a |
| Keys & cost | none (local SearXNG) | none (direct scraping) | keys, free tiers then paid |
| Engine maintenance | SearXNG community (70+ engines) | per-engine XPath scrapers, constant upstream fixes | provider's problem |
| Deadlines / cancellation | planned (Flojo `Context`) | none (blocking threads) | provider-dependent |
| Deep-research fan-out | roadmap (M2) | no — N sequential LLM calls | partial (their own APIs) |

¹ FastMCP/Python figures from [FlojoMCP benchmarks](https://github.com/CerebroCanibalus/FlojoMCP/blob/main/BENCHMARKS.md); ddgs behavior from source inspection of v9.16.0.

## Roadmap

1. **M1** — local SearXNG search, health, typed errors, docs ← *current*
2. **M2** — `research` orchestrator: parallel multi-query fan-out, dedup, progress + cancellation, compact digest (the ChatGPT/Qwen deep-research style tool)
3. **M3** — content extraction (Rust readability, no XPath) and optional cloud providers behind a `SearchProvider` trait

Decisions and progress live in [AGENTS.md](./AGENTS.md).

## License

GPL-3.0 — see [LICENSE](./LICENSE).

## Credits

- [FlojoMCP](https://github.com/CerebroCanibalus/FlojoMCP) — Rust MCP framework this server is built on.
- [SearXNG](https://github.com/searxng/searxng) — the metasearch instance doing the heavy lifting.
- [ddgs](https://github.com/deedy5/ddgs) — the Python MCP whose pain points shaped this design.
