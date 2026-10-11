# Run preparation visibility and credential readiness

## Observed defect and change

The native one-chat narrative trial appeared idle during document preparation. The backend had admitted the run, but the existing Started event arrived only after preparation. A separate credential lookup timeout was reported as a missing key, and the frontend only refreshed presence at mount. Together these could show an idle composer and stale connection hint during long preparation.

Desktop now persists and publishes the existing Started event at admission, before credential lookup, skill/MCP setup and document preparation. It suppresses the later duplicate core event; standalone core callers retain their event. Setup failures reset the running state before broadcasting the persisted Failed event. Stop remains available during setup and preparation.

Credential status returns a lookup error when its existing bounded worker times out; a successful missing-key lookup still returns false. The frontend preserves its last confirmed presence on errors and refreshes on focus, visibility return, backend reconnection and terminal run events. Concurrent triggers coalesce into one in-flight lookup and a queued follow-up. Unmounted responses cannot update state. A running composer hides the connection hint without bypassing the normal credential check.

These changes add no configuration field, registry, raw diagnostics UI or compaction command. Existing credential values remain inside the credential store.

## Regression evidence

Meaningful RED runs reproduced missing admission visibility during delayed preparation, missing start/terminal events on setup failure, the active connection hint, absent mounted readiness refreshes and a timeout incorrectly returned as `{go:false}`. Evidence logs are `/tmp/themis-preparation-start-red.log`, `/tmp/themis-admission-events-red.log`, `/tmp/themis-preparation-composer-red.log`, `/tmp/themis-readiness-red.log` and `/tmp/themis-secret-status-red.log`.

Focused attachment verification passed all 15 tests, including exactly one Started event through preparation and ordinary generation, and cancellation during preparation. Mounted readiness tests cover confirmed presence changes, preserving confirmed status on lookup error and coalescing refresh bursts. Credential admission cancellation verifies the same-run start/failure pair, idle state and no subsequent catalog setup. The timeout test uses a synthetic credential store and the real shared-server dispatch boundary; it does not access a live credential.

The read-only reviewer found no blocker in the final source and installer diffs. Rebuilt native/live proof remains pending. No live narrative or coding acceptance follows from these scripted tests.

The first fresh workspace run stopped at the MCP integration target: 411 tests passed, two timed out, two were ignored and the named OS test was filtered. The broken-connection recovery test reproduced its timeout in isolation: its 15-second timer began at the now-earlier Started event, before cold setup completed. Teardown then removed the temporary project while setup was still pending. The test now allows 60 seconds for admission/preparation, then retains its 15-second recovery bound after the actual MCP failure warning. Reviewer inspection caught an intermediate patch waiting for a nonexistent MCP ToolStarted event; it was corrected to the emitted ToolFinished warning and asserts the timer actually started. That intermediate green run is not final acceptance.

The background-connection test's 20-second readiness wait was shorter than the production connection's 30-second deadline and failed under the parallel workspace run; its unchanged isolated reproduction passed. Its wait now allows the production deadline and scheduling, retaining the 300-ms nonblocking admission assertion and immediate rejection of a healthy connection marked failed. No production MCP behavior changed. The fresh complete workspace rerun passed 442 tests with zero failures, three ignored and the named OS test filtered (`/tmp/themis-preparation-workspace-final.log`); the original failures remain in `/tmp/themis-preparation-workspace.log` and `/tmp/themis-preparation-plugins-reproduction.log`.

The isolated live project's document allowance has been saved through `save_runtime_configuration` and read back through the shared API as 1,800 seconds, with project provenance. A complete effective-config comparison confirms this was the only changed field; user configuration is unchanged. Request/batch limits remain 300/3,600 seconds, retry limit two and summary-429 base 60 seconds. Before/after snapshots are `single-thread-v1/preparation-deadline-configuration-{before,after}.json` under the external verification root. This is execution configuration, not a new narrative prompt or semantic feedback.

The staged native installer checks all entries in the persisted thread registry through live state queries, including chats without history, and verifies the registry did not change during inspection. Signature failure after replacement quarantines the candidate and restores the old bundle; successful installation retains a recoverable backup. Three isolated filesystem regressions cover rollback after signature failure, missing-stage rollback and successful backup retention. The complete external offline harness passed 34 tests (`/tmp/themis-preparation-harness-final.log`). This does not prove installation or native activity; those gates remain pending.

## Fresh checks

- `cargo test --workspace --no-fail-fast -- --skip entered_keys_survive_new_store_and_can_be_forgotten`: 442 passed, zero failed, three ignored, one filtered. This includes 15 attachment, five MCP/plugin and 13 shared-server/CLI tests. All 27 result sections were checked.
- `npm --prefix web test`: 310 passed across 30 files (`/tmp/themis-preparation-web.log`). Existing Vitest configuration deprecation and Node localStorage warnings remain.
- `npm --prefix web run build`: passed (`/tmp/themis-preparation-web-build.log`), with the existing large-chunk warning. Main assets are `index-BwbZUiXT.js` and `index-BoFMnURY.css`.
- `cargo fmt --all -- --check`, `git diff --check` and `ruff check quality_dashboard`: passed.
- `python3 -m unittest quality_dashboard.test_dashboard`: 14 passed. The running localhost dashboard was refreshed through `/api/metrics`; snapshot `/tmp/themis-preparation-dashboard-refresh.json`.
- `cargo clippy --workspace --all-targets -- -D warnings` and `npm --prefix web run lint`: passed (`/tmp/themis-preparation-clippy.log`, `/tmp/themis-preparation-web-lint-final.log`). Native inspection reported a locked Mac; an unlock was requested while independent checks continued. No new live trial has begun.

## Code health

The refreshed dashboard snapshot is `/tmp/themis-preparation-health.json`: 114 analyzed app files, mean cyclomatic complexity 2.58, cognitive complexity 2.21 and maintainability 73.2. Inline Rust tests remain included. Admission and refresh checks slightly increase touched-module branching: preferences means 1.35 to 1.37 cyclomatic, .22 to .29 cognitive and 73.5 to 72.4 maintainability; run 3.22 to 3.26, 3.30 to 3.39 and 80.4 to 80.3; threads 2.14 to 2.15, .80 to .81 and 79.4 to 79.3; store 2.44 to 2.48, 3.75 to 3.82 and 74.0 to 74.8. Composer cyclomatic changes from 3.38 to 3.46, with cognitive 2.91 and maintainability 73.7 unchanged. These branches address reproduced readiness/cancellation risks. Available Rust line coverage remains stale at 89.1% (21,023/23,586); frontend coverage is unavailable.
