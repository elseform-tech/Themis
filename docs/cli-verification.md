# Consolidated CLI verification — 2026-09-30

Implemented on `codex/cli-consolidation`, based on synchronized `main` and
`origin/main`. The three implementation commits are `69d66fe`, `b2be794`, and
`fa6f9e0`; `4105dee` carries the branch-before-work and regular-commit guidance.

The optional `themis` command uses the desktop's authenticated local app server.
It supports streamed prompts, repeated conversations, terminal tool approvals,
read-only persistence diagnostics, and hidden OpenCode Go key entry. The macOS
bundle exposes the same CLI through `--themis-cli` and an optional Unix symlink
installer. Installation refuses to overwrite an existing command and leaves
shell configuration unchanged. The independent core CLI and `themisctl` are retired.

## Automated checks

Run from the repository root unless a working directory is specified.

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed |
| `env -u OPENCODE_KEY cargo test --workspace` | Failed on the existing synthetic macOS Keychain roundtrip test |
| `env -u OPENCODE_KEY cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten` | 211 passed; one Keychain test filtered; two tests ignored, including one doctest |
| `cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten` | Passed; 85.8% Rust line coverage, 9,891 / 11,533 lines |
| `npm --prefix web run lint` | Passed |
| `npm --prefix web test` | 169 passed across 22 files |
| `npm --prefix web run build` | Passed; existing large-chunk warning remains |
| `ruff check quality_dashboard` | Passed using the existing PATH executable |
| `python3 -m unittest quality_dashboard.test_dashboard` | 13 passed; dashboard tooling scope |
| `npx -y @tauri-apps/cli@2 build --bundles app` in `crates/desktop` | Passed |
| `npx -y @tauri-apps/cli@2 build --bundles app --config '{"build":{"beforeBuildCommand":""}}'` in `crates/desktop` | Final rebuild passed, reusing the already verified frontend bundle |

The 13 CLI/server/doctor/provider end-to-end cases are part of the Rust total.
They cover shared state and restart persistence, streamed followup context and
skills, hidden key entry with Backspace, Ctrl-C cancellation and terminal echo
restoration, unattended denial and terminal approval, incomplete handoff output,
offline corruption without initialization or migration, history/context/checkpoint
consistency, live-store diagnostics, and safe installation from the desktop binary.
Credential tests use synthetic keys and an isolated in-memory store for the
positive terminal-save journey. Tests do not replace the user's OpenCode Go key.

## Separate live and packaged checks

A real CLI prompt in temporary project/data directories completed with the marker
`THEMIS_LIVE_OK`. The server inherited `OPENCODE_KEY`; its value was not inspected,
printed, copied into arguments, or stored in repository files. The isolated server
was stopped afterward. This proves one live provider request, not every model or tool.

The final executable inside `target/release/bundle/macos/Themis.app` matched the
release executable. Using that bundle in temporary directories, verification
passed for CLI symlink installation, `--version`, offline `doctor --json --deep`
without directory creation, automatic server startup, online SQLite integrity,
thread metadata, restart persistence, and shutdown. No global CLI installation
or replacement of the user's installed app was performed.

## Product health and limitations

Refreshed the dashboard snapshot and inspected AST metrics for touched modules
and the overall product using `rust-code-analysis-cli` 0.0.25. Inline Rust test
code remains in source LOC/AST scope.

| Scope | Cyclomatic average | Cognitive average | Maintainability |
| --- | ---: | ---: | ---: |
| Baseline main, 81 files | 2.13 | 1.55 | 72.6 |
| Implementation, 82 files | 2.12 | 1.56 | 73.2 |
| CLI | 4.25 | 1.47 | 80.4 |
| Doctor | 1.96 | 1.71 | 76.6 |
| Transcript store | 2.27 | 0.77 | 83.4 |
| Preferences | 1.21 | 0.00 | 74.6 |
| Desktop entry point | 2.67 | 4.50 | 61.5 |

Cognitive average increased by 0.01; the implementation adds diagnostics and
terminal control paths. No metrics are estimated and no fixed threshold is imposed.
The dashboard discovers 373 product test declarations and 13 tooling declarations;
these source counts differ from runtime totals because of parameterization and skips.
Frontend line coverage is unavailable.

The unfiltered Keychain test fails with “The user name or passphrase you entered
is not correct.” Positive OS credential-store persistence remains unverified on
this host; the terminal flow and shared storage path pass against synthetic memory
storage. Windows/Linux native credential and console behavior were not exercised
locally. The desktop GUI was not manually retested; packaged CLI acceptance is
separate from GUI acceptance. Doctor does not repair data, send provider requests,
or prove successful writes/crash durability; scheduler success is reported unverified.
