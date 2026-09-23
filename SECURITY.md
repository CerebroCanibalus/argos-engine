# Security Policy

## Supported versions

Only the latest release on `main` receives security fixes.

## Reporting a vulnerability

Please **do not** open a public issue for security vulnerabilities. Use GitHub's private reporting:

**Repository → Security → Report a vulnerability**

Include: description, affected version, reproduction steps, and impact.

Expected response time: best effort within 7 days.

## Scope notes

- Argos Engine talks to a **local SearXNG instance** by default (`127.0.0.1:8080`). Treat any config that exposes SearXNG or points `ARGOS_SEARXNG_URL` at untrusted hosts as security-sensitive.
- Never commit API keys. Future cloud providers (M3+) must read credentials from environment variables only.
- The binary has no telemetry and makes no network requests beyond the configured SearXNG URL (and, in later milestones, URLs you explicitly ask it to fetch).
