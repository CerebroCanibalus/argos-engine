# AGENTS.md — Argos Engine

Memoria viva del proyecto (flujo universal de fases: 0 inicialización → 1 fundamentos → 2 iteración → 3 funcionalidad → 4 maduración).

## Identidad y decisiones (Fases 0-1, 2026-09-23)

- **Qué es**: MCP server en Rust sobre FlojoMCP — reemplazo propio, superior y **eficiente en tokens** de los MCPs/plugins de búsqueda; research masiva estilo ChatGPT/Qwen deep research.
- **Nombre**: Argos Engine. Repo: https://github.com/CerebroCanibalus/argos-engine (GPL-3.0-only, público desde el primer commit).
- **M1 (decisión del usuario)**: **SÓLO SearXNG local** — sin API keys, sin tarjeta, sin DuckDuckGo directo, sin backends cloud. Los cloud (Serper/Tavily/Exa/Brave, verificados en Fase 0 del repo `ddgs`) quedan como adapters futuros opcionales tras un trait `SearchProvider`. **Docker es prerrequisito del M1.**
- **Referencias**: FlojoMCP en `D:\Mis Juegos\ClaudeMCPs\FlojoMCP` (dep git `https://github.com/CerebroCanibalus/FlojoMCP`, público). El MCP Python de referencia (ddgs v9.16.0) vive en `D:\Mis Juegos\ClaudeMCPs\ddgs`.

## Anti-objetivos (los problemas del MCP Python que NO repetir)

- Tools sin deadline/cancelación (`asyncio.to_thread` mudo) → aquí: timeouts en el cliente HTTP + `ctx.is_cancelled()` / `ctx.report_progress()` en el M2.
- Errores genéricos que devoran la excepción (`"Error executing tool X"`) → `ToolError::internal/invalid_params` con mensaje accionable + `.with_data(hint)`.
- Instancia fría por llamada, sin memoria de rate limits → usar `State()` de Flojo para caché y pacing.
- Investigación pesada = N llamadas secuenciales del LLM → futuro tool `research` con fan-out interno (1 llamada).
- **NDJSON siempre**: `flojo_run_stdio`. NUNCA `flojo_run_stdio_cl` con OpenCode (framing Content-Length cuelga — verificado en FlojoMCP AGENTS.md).
- stdout reservado al protocolo; logs a stderr.

## Comandos

- `build.bat` (release + tests; VsDevCmd, mata `argos-engine.exe` previo), `check.bat` (`fmt --check` + `clippy -D warnings`), `test.bat`, `fmt.bat`. Salida de scripts ASCII-only, sin `chcp 65001` (regla del ecosistema Flojo).
- Crudo: `cargo build --release`, `cargo test`, `cargo run --release`.
- **Toolchain de esta máquina (2026-09)**: VS **2026 BuildTools (18)** MSVC 14.51.36231 + SDK 10.0.26100 — la ruta `...\Microsoft Visual Studio\2022\...` de los `.bat` de FlojoMCP **NO existe aquí** (scripts de FlojoMCP rotos en este equipo; no copiarlos literalmente). `.cargo/config.toml` fija `linker` + `LIB` al toolset 18 (patrón probado de FlojoMCP); los `.bat` de Argos hacen `call` a VsDevCmd con `if exist` + fallback a la auto-detección de cargo.
- SearXNG: `docker compose -f searxng/docker-compose.yml up -d` → JSON API en `127.0.0.1:8080`. `searxng/settings.yml` habilita `search.formats: [html, json]` (sin esto el cliente recibe HTML y falla con hint); el limiter queda desactivado (por defecto lo está salvo `server.limiter: true` + Valkey).
- Calidad antes de commit: `check.bat` → `build.bat`.

## Arquitectura (estado M1 — refactor 2026-09-23)

```
src/
  main.rs        wiring: #[flojo_mcp] + flojo_run_stdio + tests FlojoTester (nada de lógica)
  config.rs      Config::from_env (ARGOS_SEARXNG_URL, timeouts) — todo env() aquí
  error.rs       enum ArgosError (InvalidQuery/Unreachable/Http/Decode/Client) →
                 From<ArgosError> for ToolError con .with_data(hint) — thiserror
  types.rs       SearchResult, Status (contratos públicos con derives)
  limits.rs      presupuestos de TOKENS: TITLE_MAX=200, SNIPPET_MAX=300, clamps limit/page
  providers/
    mod.rs       #[async_trait] trait SearchProvider { search(), health() } — M3 añade adapters sin tocar tools
    searxng.rs   SearxNgProvider: cliente reqwest compartido vía OnceLock (sin arranque frío),
                 timeouts por-request desde Config, parse_results() puro (fixtures, sin red)
  tools/
    mod.rs + status.rs + search.rs   handlers finos: validan → delegan → normalizan
tests/fixtures/searxng_search.json   contrato JSON real (campos extra ignorados), include_str! en tests
```

- **Los derives expanden rutas absolutas `::serde` / `::schemars` → `serde` (v1) y `schemars` (v0.8, la de flojo) DEBEN ser dependencias directas** en `Cargo.toml`; importar los traits vía re-exports de `flojo_mcp` (`flojo_mcp::serde`, `flojo_mcp::schemars`). Si faltan: `E0463 can't find crate for serde`, `E0433 cannot find schemars`.
- SearXNG: `GET /search?q=&format=json&page=&safesearch=`; ping 2 s / search 15 s **por request** (desde `Config`); truncado según `limits.rs` con `truncate_string` de Flojo — **semántica: corta en `max_chars` de CONTENIDO y añade `...` encima** (tope real = max+3; ver `limits::SNIPPET_MAX_CHARS`). También hay `truncate_json`, `json_bytes`, `estimate_tokens` → eficiencia de tokens.
- Dependencias: `flojo-mcp` (git), `tokio`, `serde`, `schemars`, `thiserror`, `reqwest` (rustls, sin defaults).
- Tests sin red: FlojoTester (tools) + fixtures JSON (parsing, truncado, límites, mapeo de errores).
- Roadmap: **M2** tool `research` (fan-out multi-query, dedup, progreso/cancelación, digest compacto); **M3** extracción de contenido (`libreadability`/`trafilatura` → markdown, sin XPath) y providers cloud opcionales.

## Config del cliente (opencode.jsonc)

- Nuevo server `argos`: `"command": ["D:\\Mis Juegos\\ClaudeMCPs\\argos-engine\\target\\release\\argos-engine.exe"]` (type local). Añadir cuando el M1 esté verde.
- El server `ddgs` vigente apunta al repo Python de referencia (`python -m ddgs.cli mcp` con cwd = `D:\Mis Juegos\ClaudeMCPs\ddgs`).

## Convenciones

- Rust edition 2024, rust-version 1.85+, `clippy -D warnings`, `cargo fmt` obligatorio.
- Tests con `FlojoTester` (sin transporte): identidad de `status`, rechazo de query vacía, listing de tools. Nada dependiente de red en los tests de unit; los de integración con SearXNG se marcarán `#[ignore]` o vivirán aparte.
- Inglés en README, código, mensajes de tool y commits; español en esta memoria y en las decisiones.
- Registro de decisiones: actualizar esta memoria al cerrar cada iteración/changelog.
