# Repository Guidelines

Rust, Tauri, and React/TypeScript code belong in their owning layers; preserve cross-layer behavior.

## Project Structure

- `crates/core/`: runtime, providers, tools, skills, CLI, and integration tests.
- `crates/desktop/`: Tauri app, tests, shared `tests/common/` fixtures, and icons.
- `web/src/`: React UI, state, and adjacent tests. `docs/`: project documentation.
- `quality_dashboard/`: local quality dashboard and its tests.

## Build, Test, and Development

Run from the repository root:

```sh
python3 -m pip install -r requirements-dev.txt
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm --prefix web run lint
npm --prefix web test
npm --prefix web run build
ruff check quality_dashboard
python3 -m unittest quality_dashboard.test_dashboard
npm --prefix web run dev
(cd crates/desktop && npx tauri dev)
python3 -m quality_dashboard.dashboard
```

Run checks and start local apps with these root-level commands.

## Style and Testing

Use `rustfmt`, Rust `snake_case`, and `PascalCase` types/components. Frontend uses two spaces and `camelCase`. Required linters are Clippy, ESLint (React Hooks), and Ruff. Rust tests use the built-in harness, web uses Vitest, and dashboard uses `unittest`; no coverage threshold is configured. Keep `*.test.tsx` beside frontend modules and Rust integration tests in crate `tests/`. Add focused regression cases.

Use `@Browser` for local web journeys and `@Computer` for packaged Tauri/OS journeys when available. Record repeatable scenarios and latest evidence in `quality_dashboard/manual_acceptance.json` with scope, preconditions, steps, expected results, environment, revision, and latest outcome. Include registered Browser/native E2E definitions in the overall test inventory and completion report, while keeping automated run results and manual outcomes clearly labeled by runner. After each manual run, update that scenario's latest evidence; never count an unrun or blocked case as passed. Report Browser and native E2E outcomes alongside the automated unit, integration, and E2E suites.

Before considering any task done, run fresh, relevant verification and read the complete output. For UI changes, verify the affected journey in `@Browser` or `@Computer` when available; for system-prompt changes, verify the constructed system message and the LLM request path. Report exact commands and results, and label any remaining evidence as unverified.

## Quality Dashboard

Maintain `quality_dashboard/` as the always-on product-health companion. Keep it easy to launch and aligned with source and tests. Product metrics cover `crates/` and `web/src/`, excluding dashboard/tooling. Sort all analyzed app files by LOC both ascending and descending. Inline Rust `#[cfg(test)]` code stays in source LOC/AST; label that scope. Separate dashboard self-tests. Use `rust-code-analysis-cli` 0.0.25; show unavailable analysis instead of estimates. Preserve metric definitions and the 90+ maintainability target. Test counts are not coverage.

Before marking work done, run dashboard metric tests and relevant checks, refresh it, and verify its registers. Run applicable `@Browser` and `@Computer` E2E scenarios when available, then update `quality_dashboard/manual_acceptance.json` with the outcomes and evidence. Include those scenarios in the overall test inventory and final test summary, but report their manual results separately from automated run totals. Record Browser/Computer outcomes only after running the scenario. Report failures, skips, missing coverage, and estimates accurately.

## Commits, Pull Requests, and Security

Use short imperative commit subjects such as `feat:` and `fix:`. PRs summarize changes, list checks, include relevant UI screenshots, and link issues when available. Never commit credentials or test with real ones. Secrets use macOS Keychain; OpenCode Go may read `OPENCODE_KEY` from the app environment.
