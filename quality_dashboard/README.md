# Repository Quality Dashboard

Start from the repository root:

```sh
python3 -m quality_dashboard.dashboard
```

Open `http://127.0.0.1:4178`. The server binds only to localhost. The page refreshes Git changes, 30-day churn, product source/test LOC, product test inventory, and AST complexity every five seconds. Product measurements cover `crates/` and `web/src/`; dashboard code and its tests are tooling and stay outside those totals. LOC test classification is path-based, so inline Rust `#[cfg(test)]` code remains counted with its containing source file. Test inventory totals count source-level declarations; completed-run details list runtime names, including parameterized cases when the harness reports them. Suite-level outcomes without file-level events remain unavailable for that file. Test buttons run only fixed Rust, Vitest, and Python unittest commands. Dashboard self-tests remain listed with tooling scope. A small pass/fail history is kept locally under the ignored `.quality-dashboard/` directory; test output is not saved.

Coverage is reported only from standard LCOV files at `coverage/lcov.info` or `web/coverage/lcov.info`. This repository does not currently configure coverage instrumentation, so the dashboard will show “No coverage report found” until one is generated. Test counts and test-file presence are not coverage.

The complexity register uses Mozilla's `rust-code-analysis` Tree-sitter AST analyzer on source files under `crates/` and `web/src/`; standalone test files and dashboard tooling are excluded, while inline Rust `#[cfg(test)]` modules remain in the same-file AST metrics. Install version 0.0.25 with `cargo install rust-code-analysis-cli --version 0.0.25 --locked`. If the analyzer is unavailable or fails, complexity is shown as unavailable rather than estimated. The dashboard reports per-file cyclomatic and cognitive averages, plus the mean Visual Studio maintainability index across callable AST nodes (using the module index for files without functions). By default it shows the 25 lowest-scoring app files; choose LOC ascending or descending to sort all analyzed app files. These are review signals, not correctness gates; rely on automated tests and human review for behavior.
