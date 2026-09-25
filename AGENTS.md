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
- **Auditoría de providers (medida 2026-09-24, `PROVIDER_AUDIT.md`)**: DDG y Brave acertaron 10/10 en la primera query de nicho, pero 202/429 en las siguientes; Bing devolvió HTML/SERP real pero basura semántica (`free piano VST` → juegos, `Spitfire Audio LABS free` → Macao) y no aplicó `site:` de forma fiable. Tavily/Firecrawl keyless remote dio JSON relevante, pero no son índices locales. **Decisión provisional**: no añadir más HTML providers; Bing/Brave son compatibilidad experimental hasta decidir demotion/opt-in; no evasión de anti-bot.
- **Calidad result-side (v0.2.1)**: `src/quality.rs` valida `domains`, fuerza scope exacto/subdominio y filtra deriva obvious por términos(query); `Fanout` marca `filtered`, cuenta warnings y devuelve `NoUsableResults` cuando todos los providers fallan el gate. El status `ok` ya no significa “cualquier HTML 200”.
- **FASE ACTIVA M1.2 (2026-09-25) — Native Metasearch Engine**: construir el router/registry antes de añadir providers. Objetivo: registry amplio (objetivo 30 adapters), 5–10 providers elegibles por perfil, 2–5 simultáneos por búsqueda y fallback inmediato por estado. El catálogo categorizado vive en `PROVIDERS.md`; la prioridad es `P0_KEYLESS`/`P1_PUBLIC_API`, luego vertical pública, después user-key/free-tier y por último paid/HTML experimental.
- **Routing inteligente**: `ProviderState` (healthy/cooldown/rate_limited/quota_exhausted/auth_missing/filtered/degraded/unreachable), `index_family` para no contar wrappers como diversidad, selector por perfil/calidad/cuota/coste, circuit breaker, `Retry-After`/backoff, cancelación de requests inferiores cuando hay confianza y RRF/fusión. Nunca consultar 30 providers en paralelo ni devolver el manifiesto completo en cada search.
- **Eficiencia de tokens**: separar coste de cuota (no repetir/rate-limit/cache) de payload MCP (5–10 resultados compactos, `source` hostname, budgets title/snippet, warnings sólo relevantes). La calidad se mide por éxito usable, overlap, fuentes únicas, concentración de dominios, freshness y score por categoría; HTTP200 nunca es calidad.
- **v0.3.0 (release M1.2a)**: `metasearch.rs` registry/selector, estado compartido, cooldown/rate-limit/unreachable y fallback por oleadas; cuatro adapters actuales siguen siendo los únicos con factory. RRF, `Retry-After`, cuotas y adapters keyless públicos quedan para la continuación de M1.2/M1.3.
- **Política de claves**: adapters y perfiles se distribuyen, nunca claves compartidas. Tavily/Firecrawl/Exa/Brave/Kagi/Marginalia/etc. se activan con key del usuario; providers keyless públicos son primera clase. No evasión de CAPTCHA, proxy rotation ni resolución de challenges.

## M1.2 — diseño verificable del Native Metasearch

1. **Registry primero**: `ProviderManifest`, `ProviderState`, perfiles, `index_family`, política y capacidades en configuración local; no hardcodear 30 nombres en el fanout.
2. **Selector por oleadas**: 3–5 requests iniciales, escalado inmediato si la confianza/calidad no alcanza el objetivo; no esperas secuenciales ni 30 fans indiscriminados.
3. **Estado compartido**: cooldown, `Retry-After`, backoff exponencial con jitter, quota ledger, auth-missing y circuit breaker. Un 429/202/403 debe sacar el provider de la siguiente selección.
4. **Fusión**: canonical URL dedup + RRF/weighted rank + diversidad de `index_family` + límite de concentración por dominio; conservar el mejor snippet/metadata al deduplicar.
5. **Calidad**: benchmark por categoría (general, academic, code, news), con usable-rate, unique-source-rate, overlap, domain concentration, freshness, p50/p95 y requests/result. No hacer scoring por número de HTTP 200.
6. **Tokens**: el MCP recibe 5–10 resultados compactos; los diagnósticos completos viven en `status`/herramienta de diagnóstico, no en cada búsqueda. Priorizar keyless/API pública y cachear aggressively antes de pedir paid tiers.
7. **Entrega por fases**: M1.2 registry/router; M1.3 keyless/API pública; M1.4 user-key/free-tier; M1.5 paid/local corpus. `PROVIDERS.md` es la fuente de verdad y `PROVIDER_AUDIT.md` es la evidencia viva.

## Anti-objetivos (los problemas del MCP Python que NO repetir)

- Tools sin deadline/cancelación → timeouts por-request + `ctx.is_cancelled()`/`report_progress()` en M2.
- Errores genéricos que devoran la excepción → `ArgosError` tipado + `.with_data(hint)` (ver `error.rs`; `RateLimited`/`AllProvidersRateLimited` explican el techo).
- **Fallos de providers tragados en silencio** → cada `search` devuelve `providers: Vec<ProviderEntry { name, status }>` (status = `ok|empty|rate_limited|unreachable`) + `warnings: Vec<String>`; si TODOS rate-limitean → `ArgosError::AllProvidersRateLimited` con la lista (pista la diferencia vs. "no hay hits").
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
  metasearch.rs  M1.2 native registry/selector: manifests, perfiles, estados compartidos, cooldown,
                 selección por oleadas y fallback inmediato sobre Fanout::run_selected
  quality.rs      normaliza domains, scope exacto/subdominio, construye query site: y filtro léxico conservador
  providers/
    mod.rs       trait SearchProvider {search,health} + impersonated_client() (primp ChromeV153/Windows,
                 timeout desde Config al primer uso) + impersonated_health_client() (3s, no cuelga status)
                 + compact_source(url) → host sin scheme/www/path (truncado)
    fanout.rs    Fanout::run: join_all en paralelo, quality gate result-side, merge round-robin, dedup por URL;
                 construye SearchOutcome {results, providers, warnings} reflejando el estado real de
                 cada provider (incluidos degradados/filtered); TODOS rate-limited → AllProvidersRateLimited;
                 todos los resultados filtered/irrelevantes → NoUsableResults
    duckduckgo.rs POST html/ (params ddgs: q,b,l + s=10+(page-2)*15); parse div.result → a.result__a/
                 a.result__snippet; filtra /y.js; unwrap //duckduckgo.com/l/?uddg= (percent-decode)
    bing.rs      GET /search (q,pq,cc + first vía .query); parse li.b_algo → h2 a + p; filtra aclick;
                 unwrap ck/a?u=a1<base64url> (base64 crate)
    brave.rs     GET /search (q,source=web + offset); parse div[data-type=web] → a[href] + div.title +
                 .generic-snippet .content (fixture propio337KB); ÍNDICE INDEPENDIENTE
    searxng.rs   adapter opcional JSON; auto-heal: Down + auto_start → stack::ensure_up + retry;
                 success → stack::note_usage
  tools/         handlers finos: status (probes por provider) y search (valida → append site: si domains
                 no vacío → Fanout::run)
tests/fixtures/  HTML REAL: duckduckgo.html (42KB/12 resultados), bing.html (124KB/20 bloques),
                 mojeek.html (CAPTCHA — referencia de qué NO pasar), searxng_search.json
examples/probe.rs sonda viva de endpoints
```

- **Los derives expanden rutas absolutas `::serde`/`::schemars` → `serde` (v1) y `schemars` (v0.8) DEBEN ser dependencias directas**; traits vía re-exports de flojo. Si faltan: `E0463`/`E0433`.
- Dependencias: `flojo-mcp` (git), `tokio`, `serde`, `schemars`, `thiserror`, **`primp`** (impersonation — providers keyless), `reqwest` (native-tls — sólo adapter SearXNG localhost), `scraper`, `percent-encoding`, `base64`, `futures`.
- `truncate_string(s,max)` de Flojo **corta en max de contenido y añade `...`** (tope real max+3).
- Tests sin red: FlojoTester + fixtures HTML/JSON reales + mocks del trait (fanout: interleave, dedup, límite, preferencia de error).

## Config del cliente (opencode.jsonc)

- Server `argos`: `"command": ["D:\\Mis Juegos\\ClaudeMCPs\\argos-engine\\target\\release\\argos-engine.exe"]` (type local) — **ACTIVO desde v0.1.0** (e2e verde con3 engines).
- El server `ddgs` se **MANTIENE** (no retirar): la regla global3 exige DDGS para investigación web de este asistente; además es el repo de referencia para sincronizar parsers. `argos` = el producto, `ddgs` = herramienta de research + referencia.

## Convenciones

- Rust edition 2024, rust-version **1.89** (piso de primp), `clippy -D warnings`, `cargo fmt` obligatorio.
- Inglés en README, código, mensajes de tool y commits; español en esta memoria.
- Registro de decisiones: actualizar esta memoria al cerrar cada iteración.
