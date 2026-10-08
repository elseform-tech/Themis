# Configuration and integration verification — 2026-10-08

This change uses the shared production app server for desktop and CLI. It adds
runtime JSONC, ordered Custom approval rules, per-thread YOLO, rotating JSONL
diagnostics, import compatibility reports and explicit global/project UI scope.
No compaction UI or paid provider benchmark was added.

## Automated evidence

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten --test-threads=2`: 331 passed, three ignored, one filtered.
- `cargo test -p themis-core --lib diagnostics::tests -- --test-threads=2`:
  four passed, including structured payload and environment-value redaction.
- `cargo test -p themis-desktop --test configuration_diagnostics`: four passed.
  The real CLI/server path covers config/import/log persistence, malformed-config
  repair, project symlink rejection, Custom deny versus YOLO execution, repository
  guidance in captured model requests, and legacy confirm-reads compatibility.
  Model responses use local HTTP mocks, not a paid provider.
- `cargo test -p themis-desktop --test command_coverage`: passed; the inventory
  recognizes typed frontend calls and matches the shared server command set.
- `npm --prefix web run lint`: passed.
- `npm --prefix web test -- --maxWorkers=1 --testTimeout=15000`: 285 passed
  in 27 files (258.93 seconds under concurrent verification load).
- `npm --prefix web run build`: passed; the existing >500 kB chunk warning remains.
- `ruff check quality_dashboard`: passed.
- `python3 -m unittest quality_dashboard.test_dashboard`: 13 passed.
- Local Markdown links in the updated guides and AGENTS.md: passed. The documented
  user/project JSONC examples are validated by the actual core resolver in tests.

The unfiltered Rust run reproduced the host-dependent synthetic Keychain error
(`The user name or passphrase you entered is not correct`). It is excluded by
name in the remaining-suite and coverage runs, not reported as a pass.
Two existing compaction fixtures with 300 ms deadlines failed under concurrent
compilation/test load; both passed in isolation. Production deadlines and test
assertions were unchanged. Broad reruns use `--test-threads=2` to bound load.
Public network plugin acceptance and the live provider test remain opt-in;
they were not rerun for this configuration change.

## Final UI scope

The final user-facing UI keeps only YOLO/Custom in the composer shield and
one-click plugin/skill catalog installation. Raw configuration, structured log
viewers/export, compatibility inspection dialogs, and diagnostic navigation
links were removed. MCP configuration remains an advanced file/CLI workflow.
Backend configuration, validation, revisioned integration storage, and local
logging remain available to those advanced workflows.

## Native evidence and limits

An isolated custom-protocol Tauri QA build (`ai.themis.configuration.qa`) verified
that the composer shield contains only YOLO/Custom, Settings has no configuration,
log, or diagnostic-export controls, and local fixture packages install with one
click in both User (Global) and Project scope. Production app data and the live
test were preserved. No provider request or third-party MCP process was started
for these UI checks.

The final frontend build also hides the diagnostic operation ID from installation
errors, preserving the error reason. Its regression test and production build
passed; this last display-only change was not rechecked in the native build.
Native provider execution and third-party MCP connections remain unverified.

## Code health

The refreshed dashboard analyzed 105 product files: average cyclomatic complexity
2.52, cognitive complexity 2.24, and maintainability 73.3. Inline Rust tests count
in these source metrics; dashboard tooling is excluded. There is no comparable
fresh pre-change baseline, so these values do not establish an improvement.

| Module | LOC | Cyclomatic | Cognitive | Maintainability |
| --- | ---: | ---: | ---: | ---: |
| Core configuration | 679 | 2.88 | 2.86 | 65.7 |
| Diagnostics | 443 | 2.10 | 1.11 | 72.3 |
| Desktop configuration | 246 | 2.40 | 0.75 | 78.9 |
| Plugins UI | 224 | 2.56 | 2.53 | 81.9 |
| Settings UI | 242 | 2.18 | 1.52 | 80.5 |
| Composer UI | 206 | 3.23 | 2.73 | 73.7 |

`cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten --test-threads=2`
passed: 331 tests, two ignored live/network tests, one filtered Keychain test.
This coverage invocation does not include the illustrative ignored doctest counted
by the ordinary workspace run. The refreshed dashboard reports Rust line coverage
of **88.2% (18,575 / 21,060 lines)**. This is Rust coverage only; frontend test
counts are not coverage. No coverage threshold or comparable fresh baseline was
used to claim a gain.
