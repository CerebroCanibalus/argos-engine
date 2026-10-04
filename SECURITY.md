# Security Policy

## Supported versions

Only the latest release on `main` receives security fixes.

## Reporting a vulnerability

Please **do not** open a public issue for security vulnerabilities. Use GitHub's private reporting:

**Repository → Security → Report a vulnerability**

Include: description, affected version, reproduction steps, and impact.

Expected response time: best effort within 7 days.

## Scope notes

- Argos Engine sends your **query text** to the configured public search
  providers over HTTPS (DuckDuckGo, Bing, Brave and the keyless public APIs by
  default). There is no local index: a search is only private to the extent
  those providers make it so. Pointing `ARGOS_PROVIDERS` at a different set
  changes where queries go.
- Optional credentials (`ARGOS_GITHUB_TOKEN`, `ARGOS_SEMANTIC_SCHOLAR_KEY`,
  `ARGOS_CONTACT_EMAIL`) are read from environment variables only. They must
  never be committed, logged, or echoed into an error hint.
- The binary has no telemetry. Its network traffic is limited to the providers
  listed in `ARGOS_PROVIDERS`.
