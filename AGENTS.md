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

Exercise backend features through the local app server and `themis` in isolated automated tests. Keep UI and OS verification separate from the dashboard's automated test inventory. Add a focused CLI end-to-end case for a changed backend flow and report any unverified UI behavior explicitly.

Before considering any task done, run fresh, relevant verification and read the complete output. For UI changes, verify the affected journey in a browser or native app when available; for system-prompt changes, verify the constructed system message and the LLM request path. Report exact commands and results, and label any remaining evidence as unverified.

## Quality Dashboard

Maintain `quality_dashboard/` as the always-on product-health companion. Keep it easy to launch and aligned with source and tests. Product metrics cover `crates/` and `web/src/`, excluding dashboard/tooling. Sort all analyzed app files by LOC both ascending and descending. Inline Rust `#[cfg(test)]` code stays in source LOC/AST; label that scope. Separate dashboard self-tests. Use `rust-code-analysis-cli` 0.0.25; show unavailable analysis instead of estimates. Preserve metric definitions without imposing a fixed maintainability threshold. Generate Rust line coverage with open-source `cargo-llvm-cov`; test counts are not coverage.

Before marking work done, run dashboard metric tests and relevant checks, refresh it, and inspect code health and complexity for the touched modules and overall product. Review cyclomatic and cognitive complexity, maintainability, and available line coverage; make only reasonable changes that improve clarity or address a concrete risk, and call out regressions or missing metrics rather than inventing values. Do not trim working code merely to raise a score. Run relevant CLI end-to-end checks and report their outcomes with the automated test inventory. Report failures, skips, missing coverage, and estimates accurately.

## Commits, Pull Requests, and Security

Before making changes for a new task, create a dedicated `codex/<task-name>` branch from up-to-date `main`. When continuing an existing task, use its branch. Check the working tree first and preserve unrelated local changes; never implement directly on `main`.

Commit regularly as each focused task or logical subtask is completed and relevant checks pass. Keep commits small and reviewable, include the corresponding tests and documentation, inspect the staged diff before committing, and stage only changes belonging to that task. Do not defer all work to one large final commit.

Use short imperative commit subjects such as `feat:` and `fix:`. PRs summarize changes, list checks, include relevant UI screenshots, and link issues when available. Never commit credentials or test with real ones. Secrets use macOS Keychain; OpenCode Go may read `OPENCODE_KEY` from the app environment.
