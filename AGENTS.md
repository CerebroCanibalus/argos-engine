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

## Arquitectura (estado M1)

- `src/main.rs`: servidor `#[flojo_mcp(name = "argos-engine", version = ...)]` + tools `status` y `search`. Tipos de salida con derives `Serialize, Deserialize, JsonSchema`. **Los derives expanden rutas absolutas `::serde` / `::schemars` → `serde` (v1) y `schemars` (v0.8, la misma que flojo) DEBEN ser dependencias directas** en `Cargo.toml`; importar los traits vía re-exports de `flojo_mcp` (`flojo_mcp::serde`, `flojo_mcp::schemars`). Errores si se omiten: `E0463 can't find crate for serde`, `E0433 cannot find schemars`.
- `src/searxng.rs`: cliente `GET /search?q=&format=json&page=&safesearch=`; env `ARGOS_SEARXNG_URL` (default `http://127.0.0.1:8080`); timeouts: 15 s request / 2 s ping. Snippets truncados con `truncate_string` (prelude de Flojo; también hay `truncate_json`, `json_bytes`, `estimate_tokens` → herramientas de eficiencia de tokens).
- Dependencias mínimas: `flojo-mcp` (git), `tokio`, `reqwest` (rustls, sin defaults).
- Roadmap: **M2** tool `research` (fan-out multi-query, dedup, progreso/cancelación, digest compacto); **M3** extracción de contenido (`libreadability`/`trafilatura` → markdown, sin XPath) y providers cloud opcionales.

## Config del cliente (opencode.jsonc)

- Nuevo server `argos`: `"command": ["D:\\Mis Juegos\\ClaudeMCPs\\argos-engine\\target\\release\\argos-engine.exe"]` (type local). Añadir cuando el M1 esté verde.
- El server `ddgs` vigente apunta al repo Python de referencia (`python -m ddgs.cli mcp` con cwd = `D:\Mis Juegos\ClaudeMCPs\ddgs`).

## Convenciones

- Rust edition 2024, rust-version 1.85+, `clippy -D warnings`, `cargo fmt` obligatorio.
- Tests con `FlojoTester` (sin transporte): identidad de `status`, rechazo de query vacía, listing de tools. Nada dependiente de red en los tests de unit; los de integración con SearXNG se marcarán `#[ignore]` o vivirán aparte.
- Inglés en README, código, mensajes de tool y commits; español en esta memoria y en las decisiones.
- Registro de decisiones: actualizar esta memoria al cerrar cada iteración/changelog.
