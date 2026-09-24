# AGENTS.md — Argos Engine

Memoria viva del proyecto (flujo universal de fases: 0 inicialización → 1 fundamentos → 2 iteración → 3 funcionalidad → 4 maduración).

## Identidad y decisiones

- **Qué es**: MCP server en Rust sobre FlojoMCP — reemplazo propio, superior y **eficiente en tokens** de los MCPs/plugins de búsqueda; research masiva estilo ChatGPT/Qwen.
- **Nombre**: Argos Engine. Repo: https://github.com/CerebroCanibalus/argos-engine (GPL-3.0-only, público desde el primer commit).
- **M1 FINAL (2026-09-24, decisión del usuario tras datos)**: **fanout multi-provider keyless — DuckDuckGo + Bing + Brave por defecto** (`ARGOS_PROVIDERS`), **cero VM/cero Docker/cero keys**. Transporte = **primp** (crate de deedy5, el mismo que ddgs) **impersonando ChromeV153/Windows** — sin eso Bing redirige a su homepage (302) y Brave devuelve429. El requisito inicial "Sólo SearXNG" se abandonó tras medir el coste real de la VM (ver saga WSL). SearXNG queda como **adapter opcional** ya escrito (`providers/searxng.rs` + `stack.rs`, sigue en reqwest — localhost no necesita impersonation).
- **Referencias**: FlojoMCP en `D:\Mis Juegos\ClaudeMCPs\FlojoMCP` (dep git, público). MCP Python de referencia (ddgs v9.16.0) en `D:\Mis Juegos\ClaudeMCPs\ddgs` — parsers de motores para sincronizar fixes.

## Datos de límites (medidos,2026-09-24, esta IP/residencial)

- `html.duckduckgo.com`: **200 con12 resultados** usando reqwest+rustls+UA Chrome (UA de PowerShell → **403**: la UA importa). Avisos públicos:202/403 bajo automatización.
- `www.bing.com/search`: **200**,20 bloques `b_algo` (URLs envueltas en `ck/a?u=a1<base64url>`).
- `www.mojeek.com`: **página CAPTCHA** en todo caso (2 intentos planos + primp; es bloqueo IP/consent) → FUERA del set. `search.brave.com`: **429 con clientes planos,200 con primp** → DENTRO del set.
- **SearXNG NO exenta**: docs oficiales dicen que es clasificado como bot y recibe CAPTCHA/bloqueo (su limiter te frena a ti). Issue: motores suspendidos24h.
- **Transporte = el discriminante real** (medido2026-09-24): reqwest puro (rustls **y** schannel/native-tls) → Bing **302 a homepage** (mismo header set y HTTP/1.1 que curl, que sí pasaba → delta = ClientHello/JA3), Brave **429**, Mojeek CAPTCHA; PowerShell → DDG403. **primp `Impersonate::ChromeV153` + `ImpersonateOS::Windows` → DDG/Bing/Brave todos200 con resultados reales**; Mojeek sigue CAPTCHA (bloqueo IP/consent, no TLS). `rust-version` subido a **1.89** (piso de primp). `aws-lc-sys` compila OK con VS18 (cmake crate).
- ddgs marca `provider="bing"` en DuckDuckGo → DDG y Bing comparten índice pero son **presupuestos anti-bot independientes**; **Brave aporta índice INDEPENDIENTE** (crawler propio) → diversidad real +3º presupuesto.
- **Lección**: el techo se sube con diseño (fanout paralelo + dedup + pacing + cache), no con un backend mágico.

## Anti-objetivos (los problemas del MCP Python que NO repetir)

- Tools sin deadline/cancelación → timeouts por-request + `ctx.is_cancelled()`/`report_progress()` en M2.
- Errores genéricos que devoran la excepción → `ArgosError` tipado + `.with_data(hint)` (ver `error.rs`; `RateLimited` explica el techo).
- Instancia fría por llamada → clientes compartidos (`OnceLock`) + caché en M2.
- N llamadas secuenciales del LLM → futuro tool `research` (fan-out interno).
- **NDJSON siempre**: `flojo_run_stdio`, NUNCA `flojo_run_stdio_cl` con OpenCode. stdout sólo protocolo.

## Comandos

- `build.bat` (release + tests; VsDevCmd VS18 con `if exist` + fallback), `check.bat` (`fmt --check` + `clippy -D warnings`), `test.bat`, `fmt.bat`. Salida ASCII-only, sin `chcp` (regla Flojo).
- Crudo: `cargo build --release`, `cargo test`, `cargo clippy --all-targets -- -D warnings`.
- `cargo run --example probe` — sonda viva DDG/Mojeek (diagnóstico de bloqueos).
- Calidad antes de commit: `check.bat` → `build.bat`.
- **El camino por defecto NO necesita Docker ni WSL.** Adapter SearXNG (opcional): `searxng\wsl-setup.bat` (una vez, admin) o `docker compose -f searxng/docker-compose.yml up -d`; diario `searxng\wsl-up.bat`.

## Saga WSL (cerrada como adapter opcional — contexto para no repetirla)

- Docker Desktop **imposible** aquí (LTSC **19044** < requerido19045, sin feature updates). `wsl --update` del wsl in-box = **no-op** (exit0 miente); kernel arreglado con **MSI oficial** `wsl_update_x64.msi` (vía blob wslstorestorage, `aka.ms/wsl2kernel` devuelve HTML). WSL empaquetado2.7.14 **gateado por OS** (`WSL_E_OS_NOT_SUPPORTED`, pide CU — el SO lleva ~1 año sin updates; `aka.ms/store-wsl-kb-win10` → historia de updates; último CU .7727 vs actual .1288).
- Estado: kernel in-box INSTALADO ✓, Alpine rootfs3.22.6 descargado+SHA256 ✓ (`.temp/alpine-rootfs.tar.gz`), **faltaría** `wsl --import` + adaptar `wsl-setup.sh` a apk — sólo si alguien reactiva el adapter.
- Gotchas cmd: `wsl.exe` devuelve **-1** → NUNCA `if errorlevel1` (signed), usar `!errorlevel! neq0` con `setlocal enabledelayedexpansion`; conversión ruta `D:\a\b`→`/mnt/d/a/b` **testeada** (bug clásico: `!REST:\=/!` necesita el `!` de cierre); PATH stale: invocar `C:\Program Files\WSL\wsl.exe` por ruta absoluta.
- `.temp/`: `wsl.msi` (WSL moderno259MB), `wsl_update_x64.msi` (kernel17MB), `alpine-rootfs.tar.gz`, `install-docker.bat` (obsoleto). Todo gitignored.

## Arquitectura (M1 multi-provider)

```
src/
  main.rs        wiring: #[flojo_mcp] + flojo_run_stdio + tests FlojoTester
  config.rs      Config::from_env — ARGOS_PROVIDERS, ARGOS_SEARXNG_URL, timeouts, lifecycle
  error.rs       enum ArgosError (InvalidQuery/Down/Unreachable/RateLimited/Http/Decode/StackBoot/StackStarting)
                 → From<ArgosError> for ToolError con .with_data(hint) — thiserror, Clone
  types.rs       SearchResult, Status, ProviderHealth (contratos con derives)
  limits.rs      presupuestos de TOKENS: TITLE_MAX=200, SNIPPET_MAX=300, clamps limit/page
  stack.rs       ciclo de vida WSL2 on-demand (sólo adapter SearXNG): ensure_up/note_usage/watchdog
  providers/
    mod.rs       trait SearchProvider {search,health} + impersonated_client() (primp ChromeV153/Windows,
                 timeout desde Config al primer uso) + impersonated_health_client() (3s, no cuelga status)
    fanout.rs    Fanout: join_all en paralelo, merge round-robin, dedup por URL normalizada,
                 si todo falla prefiere el error RateLimited; from_config ignora nombres desconocidos
                 y cae a los defaults si la lista queda vacía
    duckduckgo.rs POST html/ (params ddgs: q,b,l + s=10+(page-2)*15); parse div.result → a.result__a/
                 a.result__snippet; filtra /y.js; unwrap //duckduckgo.com/l/?uddg= (percent-decode)
    bing.rs      GET /search (q,pq,cc + first vía .query); parse li.b_algo → h2 a + p; filtra aclick;
                 unwrap ck/a?u=a1<base64url> (base64 crate)
    brave.rs     GET /search (q,source=web + offset); parse div[data-type=web] → a[href] + div.title +
                 .generic-snippet .content (fixture propio337KB); ÍNDICE INDEPENDIENTE
    searxng.rs   adapter opcional JSON; auto-heal: Down + auto_start → stack::ensure_up + retry;
                 success → stack::note_usage
  tools/         handlers finos: status (probes por provider) y search (valida → Fanout)
tests/fixtures/  HTML REAL: duckduckgo.html (42KB/12 resultados), bing.html (124KB/20 bloques),
                 mojeek.html (CAPTCHA — referencia de qué NO pasar), searxng_search.json
examples/probe.rs sonda viva de endpoints
```

- **Los derives expanden rutas absolutas `::serde`/`::schemars` → `serde` (v1) y `schemars` (v0.8) DEBEN ser dependencias directas**; traits vía re-exports de flojo. Si faltan: `E0463`/`E0433`.
- Dependencias: `flojo-mcp` (git), `tokio`, `serde`, `schemars`, `thiserror`, **`primp`** (impersonation — providers keyless), `reqwest` (native-tls — sólo adapter SearXNG localhost), `scraper`, `percent-encoding`, `base64`, `futures`.
- `truncate_string(s,max)` de Flojo **corta en max de contenido y añade `...`** (tope real max+3).
- Tests sin red: FlojoTester + fixtures HTML/JSON reales + mocks del trait (fanout: interleave, dedup, límite, preferencia de error).

## Config del cliente (opencode.jsonc)

- Server `argos`: `"command": ["D:\\Mis Juegos\\ClaudeMCPs\\argos-engine\\target\\release\\argos-engine.exe"]` — añadir cuando el e2e verde cierre M1; retirar el server `ddgs` entonces (su cwd apunta al repo Python de referencia).

## Convenciones

- Rust edition 2024, rust-version **1.89** (piso de primp), `clippy -D warnings`, `cargo fmt` obligatorio.
- Inglés en README, código, mensajes de tool y commits; español en esta memoria.
- Registro de decisiones: actualizar esta memoria al cerrar cada iteración.
