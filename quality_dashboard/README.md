# Repository Quality Dashboard

Start from the repository root:

```sh
python3 -m quality_dashboard.dashboard
```

Open `http://127.0.0.1:4178`. The server binds only to localhost. The page refreshes Git changes, 30-day churn, product source/test LOC, product test inventory, and AST complexity every five seconds. Product measurements cover `crates/` and `web/src/`; dashboard code and its tests are tooling and stay outside those totals. LOC test classification is path-based, so inline Rust `#[cfg(test)]` code remains counted with its containing source file. Test inventory totals count source-level declarations; completed-run details list runtime names, including parameterized cases when the harness reports them. Suite-level outcomes without file-level events remain unavailable for that file. Test buttons run only fixed Rust, Vitest, and Python unittest commands. Dashboard self-tests remain listed with tooling scope. A small pass/fail history is kept locally under the ignored `.quality-dashboard/` directory; test output is not saved.

Rust line coverage comes from the open-source [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) tool. Install it with `cargo install cargo-llvm-cov --version 0.9.1 --locked` and `rustup component add llvm-tools-preview`, then run from the repository root:

```sh
mkdir -p coverage
cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten
```

The dashboard reads the generated Rust report at `coverage/lcov.info` and, when present, a separate frontend report at `web/coverage/lcov.info`. It labels each source separately and does not infer coverage from test counts. The Rust report excludes the host-dependent Keychain test, test files, and doctests; coverage is a line metric for instrumented Rust source, not proof of native UI behavior. The CI coverage job uploads its LCOV report as an artifact; download it to `coverage/lcov.info` to inspect that run locally.

The complexity register uses Mozilla's `rust-code-analysis` Tree-sitter AST analyzer on source files under `crates/` and `web/src/`; standalone test files and dashboard tooling are excluded, while inline Rust `#[cfg(test)]` modules remain in the same-file AST metrics. Install version 0.0.25 with `cargo install rust-code-analysis-cli --version 0.0.25 --locked`. If the analyzer is unavailable or fails, complexity is shown as unavailable rather than estimated. The dashboard reports per-file cyclomatic and cognitive averages, plus the mean Visual Studio maintainability index across callable AST nodes (using the module index for files without functions). By default it shows the 25 lowest-scoring app files; choose LOC ascending or descending to sort all analyzed app files. Before finishing a change, inspect touched modules and the overall metrics, making only reasonable changes that improve clarity or address a concrete risk. These are review signals, not correctness gates; rely on automated tests and human review for behavior.

The manual E2E register groups product journeys by feature tags from `manual_acceptance.json`; dashboard tooling journeys appear separately. For each new or changed product feature, add or update an appropriate `@Browser` or `@Computer` journey and tag its `covers` list before executing it. Include the planned environment, steps, and expected results, and mark new or revised journeys `not-run`. Update `latest` only after observing the run. Native scenarios cover the full product and agent conversation with synthetic fixtures; a listed but unrun journey is planned coverage, not a passed test.
