# Argos Provider Registry

**Estado:** catálogo de diseño para M1.2 Native Metasearch Engine  
**Fecha de investigación:** 2026-09-25  
**Fuente principal:** documentación oficial de cada proveedor y motor documentado por SearXNG.

Este documento es la fuente de verdad para priorizar adapters. No significa que
todos los adapters estén implementados. La fecha de un free tier, cuota o precio
no es una garantía: Argos debe verificarlo periódicamente y reportar el estado
real de cada provider.

## Prioridad y política

| Política | Significado |
|---|---|
| `P0_KEYLESS` | Sin API key y útil como fallback local; priorísimo |
| `P1_PUBLIC_API` | API pública/JSON estable, normalmente sin key; alta prioridad |
| `P1_VERTICAL` | Excelente para una categoría, no para web general |
| `P2_USER_KEY` | Se activa sólo si el usuario configura su propia clave |
| `P3_PAID` | Provider de pago; nunca es el primer fallback |
| `HTML_BETA` | HTML frágil, parser/pacing/anti-bot; sólo experimental |
| `WRAPPER` | Puede funcionar, pero no cuenta como índice independiente |
| `LEGACY` | No iniciar nuevas integraciones |

> **Key/token de provider** es distinto de los tokens de salida del LLM. La
> prioridad “sin token” significa no exigir una API key al usuario.

## Registro inicial: 30 adapters

### A. General web

| # | Adapter | Categoría | Auth | Coste/tier | Política | Notas |
|---:|---|---|---|---|---|---|
| 1 | `duckduckgo_html` | Web general | ninguna | $0; bloqueos 202/403 | `HTML_BETA` | JSON no oficial; parcialmente basado en Bing |
| 2 | `bing_html` | Web general | ninguna | $0 | `HTML_BETA`, bajo peso | Cobertura, no diversidad; API oficial retirada |
| 3 | `brave_html` | Web general | ninguna | $0; inestable | `HTML_BETA` | Índice independiente, HTML bloqueable |
| 4 | `brave_api` | Web general | `BRAVE_API_KEY` | Free tier documentado; paid después | `P2_USER_KEY` | Preferible a `brave_html` |
| 5 | `qwant` | Web regional | ninguna | sin contrato estable | `HTML_BETA` | API no documentada; Datadome/cookies |
| 6 | `mwmbl_api` | Small/independent web | none/key opcional |Cuota anónima limitada; key mejora cuota | `P1_PUBLIC_API` | Índice pequeño, libre, no-profit |
| 7 | `marginalia_api` | Blogs/small web | `public`/key | Key personal no-comercial; paid comercial | `P1_VERTICAL` | Índice independiente, baja cobertura |
| 8 | `wiby_json` | Classic/small web | ninguna | $0; sin SLA | `P1_VERTICAL` | JSON público, atribución requerida |
| 9 | `yep_api` | Web general | `YEP_API_KEY` | Free tier documentado | `P2_USER_KEY` | crawling Ahrefs; revisar calidad propia |
| 10 | `mojeek_api` | Web general | `MOJEEK_API_KEY` | Paid; HTML no es fallback estable | `P2_USER_KEY` | Índice independiente |
| 11 | `kagi_api` | General/small web | `KAGI_API_KEY` | Paid; sin free tier confirmado | `P3_PAID` | Calidad premium, no primera opción |
| 12 | `yandex_api` | Russian/CIS | key + folder | Billing/quotas Yandex | `P2_USER_KEY` | Activar por locale |
| 13 | `tavily_search` | Agentic web | keyless/key | Keyless limitado; free tier con key | `P2_USER_KEY` | Ranking orientado a agentes; remoto |
| 14 | `exa_search` | Semantic/neural web | `EXA_API_KEY` | Créditos iniciales/free tier | `P2_USER_KEY` | Alta relevancia semántica; remoto |
| 15 | `serpapi_google` | Google SERP relay | `SERPAPI_API_KEY` | Free tier limitado; paid | `P2_USER_KEY`, `WRAPPER` | JSON estable, no índice propio |
| 16 | `perplexity_search` | Fresh/agentic web | `PERPLEXITY_API_KEY` | Paid | `P3_PAID` | Ranking propietario; revisar contrato |
| 17 | `google_cse_legacy` | Programmable Search | key + CX | Legacy; cierre anunciado 2027 | `LEGACY` | No iniciar integración |

### B. Academic, knowledge y news

| # | Adapter | Categoría | Auth | Política | Notas |
|---:|---|---|---|---|---|
| 18 | `openalex` | Academic | key opcional | `P1_PUBLIC_API` | Papers, DOI, abstracts, citas, OA |
| 19 | `semantic_scholar` | Academic | key opcional | `P1_PUBLIC_API` | Discovery, citations, papers relacionados |
| 20 | `crossref` | Academic metadata | ninguna | `P1_PUBLIC_API` | DOI y metadata canónica |
| 21 | `europe_pmc` | Biomed/life sciences | ninguna | `P1_VERTICAL` | REST/OAI/full-text parcial |
| 22 | `pubmed_eutils` | Biomed | key opcional | `P1_VERTICAL` | Alta precisión vertical |
| 23 | `arxiv` | Preprints | ninguna | `P1_PUBLIC_API` | API pública; aplicar pacing |
| 24 | `doaj` | Open access | ninguna | `P1_PUBLIC_API` | Journals y artículos OA |
| 25 | `gdelt_doc` | Global news | ninguna | `P1_VERTICAL` | Tres meses de news, URLs/fechas/idioma |

### C. Código, paquetes y local

| # | Adapter | Categoría | Auth | Política | Notas |
|---:|---|---|---|---|---|
| 26 | `github_search` | Code/repos | token opcional; code requiere token | `P1_VERTICAL` | Repo/issues keyless limitado; code autenticado |
| 27 | `sourcegraph` | Cross-repo code | token | `P2_USER_KEY` | Alta calidad de búsqueda de código |
| 28 | `package_registries` | npm/crates/Packagist/Maven/Repology | normalmente ninguna | `P1_VERTICAL` | Implementar como familia, no como un fetch único |
| 29 | `wikimedia_knowledge` | Wikipedia/Wikidata/Open Library | ninguna | `P1_VERTICAL` | Entidades, aliases, libros; no web ranking |
| 30 | `local_search` | Self-hosted | según backend | `LOCAL_ONLY` | YaCy, Meilisearch, ES, Solr, SQLite/FTS |

## Perfiles iniciales

```toml
[profiles.general]
providers = ["duckduckgo_html", "mwmbl_api", "tavily_search", "brave_api", "firecrawl", "exa_search"]

[profiles.academic]
providers = ["openalex", "crossref", "arxiv", "semantic_scholar", "europe_pmc", "pubmed_eutils", "doaj"]

[profiles.code]
providers = ["github_search", "sourcegraph", "package_registries", "gitlab", "gitea"]

[profiles.news]
providers = ["gdelt_doc", "tavily_search", "brave_api"]

[profiles.local]
providers = ["local_search", "searxng", "wikimedia_knowledge"]
```

`firecrawl`, `gitlab`, `gitea` y `searxng` pueden estar disponibles aunque no
sean clave-less; el selector sólo los usa si la configuración del usuario los
habilita.

## Wrappers y duplicados

No contar como diversidad independiente:

- `google`, `google_news`, `google_images`, `google_videos`: una familia Google.
- `bing`, `bing_news`, `bing_images`, `bing_videos`: una familia Bing.
- `startpage`: resultados de Google con privacidad; wrapper.
- `ecosia`: Bing/Google/EUSP; wrapper regional.
- `yahoo`: dependencia histórica de Bing, sin ventaja contractual clara.
- DDG: tiene fuentes propias, pero sus resultados tradicionales son
  parcialmente Bing; medirlo como `ddg_mixed`, no como Bing puro.

## Routing por estado

Cada adapter tendrá estado runtime:

```text
healthy
cooldown
rate_limited
quota_exhausted
auth_missing
filtered
degraded
unreachable
```

El selector **no espera** a un provider en cooldown. Si un provider recibe 429,
202, 403, quota exhaustion o auth missing:

1. se registra el evento;
2. se calcula `cooldown_until` o `quota_exhausted`;
3. se marca el adapter no elegible;
4. se lanza el siguiente provider elegible;
5. si una ola completa falla, se abre la siguiente.

## Oleadas y concurrencia

No se consultan los 30 adapters en cada búsqueda.

```text
Wave 1: 3–5 providers elegibles de mayor score
Wave 2: fallback si no se alcanza confianza
Wave 3: vertical/paid sólo si la política lo permite
```

El score combina:

```text
calidad previa por categoría
+ diversidad de index_family
+ disponibilidad
+ cuota
+ latencia
+ política del provider
```

Si se alcanza el objetivo de confianza y resultados útiles, se cancelan las
peticiones de menor prioridad. El agente nunca recibe 30 respuestas crudas.

## Eficiencia de tokens del MCP

La eficiencia se mide en dos capas distintas:

1. **Coste de consulta:** no pedir 30 resultados, no repetir providers, usar
   cache y cooldown.
2. **Coste de contexto:** devolver 5–10 resultados compactos, `source` como
   hostname, título/snippet con budgets existentes y sólo warnings relevantes.

El registry no debe aumentar el payload MCP con el manifiesto completo. Los
detalles de providers deben vivir en `status`/diagnóstico, no en cada `search`.

## Orden de implementación

### M1.2 — Registry y routing

- `ProviderManifest`, `ProviderState`, perfiles y `index_family`.
- Circuit breaker, cooldown, quotas y cancelación.
- `providers.toml` y tests del selector.
- RRF/fusión y métricas de contribución.

### M1.3 — Keyless/API adapters

Prioridad:

```text
OpenAlex
Crossref
arXiv
Semantic Scholar
Europe PMC
PubMed
DOAJ
GDELT
Wikipedia/Wikidata
Mwmbl
Wiby
npm
crates.io
Packagist
Maven
Tavily keyless
```

### M1.4 — User-key/free-tier adapters

```text
Brave API
Tavily
Firecrawl
Exa
Mwmbl key
Marginalia
Yep
Mojeek API
SerpApi
Sourcegraph
```

### M1.5 — Paid/local adapters

```text
Kagi
Yandex API
Perplexity
YaCy
Meilisearch
Elasticsearch
Solr
SQLite/Tantivy local corpus
```

## Fuentes oficiales

- SearXNG engine registry: <https://docs.searxng.org/dev/engines/index.html>
- SearXNG engine configuration: <https://docs.searxng.org/admin/settings/settings_engines.html>
- OpenAlex: <https://docs.openalex.org/>
- Semantic Scholar API: <https://www.semanticscholar.org/product/api>
- Crossref REST API: <https://www.crossref.org/documentation/retrieve-metadata/rest-api/>
- Europe PMC: <https://europepmc.org/developers>
- PubMed: <https://www.ncbi.nlm.nih.gov/home/develop/api>
- arXiv API: <https://info.arxiv.org/help/api/user-manual.html>
- GDELT DOC 2.0: <https://blog.gdeltproject.org/gdelt-doc-2-0-api-debuts/>
- GitHub Search: <https://docs.github.com/en/rest/search/search>
- Tavily keyless: <https://docs.tavily.com/documentation/keyless>
- Firecrawl Search: <https://docs.firecrawl.dev/features/search>
- Exa Search: <https://exa.ai/docs/reference/search>
- Brave Search API: <https://api-dashboard.search.brave.com/documentation/>
- Kagi API: <https://help.kagi.com/kagi/api/api-portal.html>
- Common Crawl: <https://index.commoncrawl.org/>
- Internet Archive Search: <https://archive.org/developers/search.html>
