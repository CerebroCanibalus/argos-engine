# Contributing to Argos Engine

Thanks for your interest in contributing.

## Workflow

1. Fork and create a feature branch from `main`.
2. Make your changes with tests.
3. Run quality gates before opening a PR:
   ```powershell
   check.bat   # cargo fmt --check + cargo clippy -D warnings
   build.bat   # cargo build --release + cargo test
   ```
4. Open a PR using the template. Keep commits focused and described in English.

## Guidelines

- Rust edition 2024, `rust-version = 1.89` (primp's floor). `clippy -D warnings` must pass.
- Public items need doc comments; keep tool descriptions short and agent-friendly.
- Tool errors must be actionable: real message + hint, never generic text.
- Token efficiency is a feature: prefer compact payloads, truncation, dedup.
- No API keys in core: the default path is keyless public providers. Keyed providers stay opt-in behind the `SearchProvider` trait.
- Decision changes go through issues first, then update `AGENTS.md`.

## Reporting bugs

Use the [bug report template](.github/ISSUE_TEMPLATE/bug_report.md). Include engine version, OS, `ARGOS_PROVIDERS`, and exact tool error output.
