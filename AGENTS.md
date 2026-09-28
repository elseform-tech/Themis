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
mkdir -p coverage
cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten
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

Use `@Browser` for local web journeys and `@Computer` for packaged Tauri/OS journeys when available. Organize manual E2E around product features. When adding or changing a feature, add or update at least one appropriate Browser or Computer journey in `quality_dashboard/manual_acceptance.json` and tag its `covers` list with the feature (for example settings, automations, skills, agent-loops, or conversation). The dashboard derives its feature register from these tags, so new feature tags appear automatically. Before running a journey, record its ID, scope, runner, target, covered features, planned environment, preconditions, steps, and expected results. Set new or materially revised journeys to `not-run` before execution. After the run, record its actual outcome, date, revision, environment, and evidence notes. Never carry a previous pass over to a changed journey or count an unrun or blocked case as passed. Include registered Browser/native E2E definitions in the overall test inventory and completion report, while keeping automated run results and manual outcomes clearly labeled by runner.

Before considering any task done, run fresh, relevant verification and read the complete output. For UI changes, verify the affected journey in `@Browser` or `@Computer` when available; for system-prompt changes, verify the constructed system message and the LLM request path. Report exact commands and results, and label any remaining evidence as unverified.

## Quality Dashboard

Maintain `quality_dashboard/` as the always-on product-health companion. Keep it easy to launch and aligned with source and tests. Product metrics cover `crates/` and `web/src/`, excluding dashboard/tooling. Sort all analyzed app files by LOC both ascending and descending. Inline Rust `#[cfg(test)]` code stays in source LOC/AST; label that scope. Separate dashboard self-tests. Use `rust-code-analysis-cli` 0.0.25; show unavailable analysis instead of estimates. Preserve metric definitions without imposing a fixed maintainability threshold. Generate Rust line coverage with open-source `cargo-llvm-cov`; test counts are not coverage.

Before marking work done, run dashboard metric tests and relevant checks, refresh it, and inspect code health and complexity for the touched modules and overall product. Review cyclomatic and cognitive complexity, maintainability, and available line coverage; make only reasonable changes that improve clarity or address a concrete risk, and call out regressions or missing metrics rather than inventing values. Do not trim working code merely to raise a score. Run applicable pre-registered `@Browser` and `@Computer` E2E scenarios when available, then update `quality_dashboard/manual_acceptance.json` with the outcomes and evidence. Include those scenarios in the overall test inventory and final test summary, but report their manual results separately from automated run totals. Report failures, skips, missing coverage, and estimates accurately.

## Commits, Pull Requests, and Security

Use short imperative commit subjects such as `feat:` and `fix:`. PRs summarize changes, list checks, include relevant UI screenshots, and link issues when available. Never commit credentials or test with real ones. Secrets use macOS Keychain; OpenCode Go may read `OPENCODE_KEY` from the app environment.
