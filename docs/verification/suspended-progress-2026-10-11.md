# Failed-checkpoint progress verification — 2026-10-11

Required compaction can fail after tools have already changed files. Previously, a later ordinary request rebuilt from durable conversation turns without those failed-run tool observations. Exact originals remained archived, but the first continuation packet did not directly carry the completed action. That made repeated work possible even when the original task survived.

The runtime now publishes a partial historical observation record after a noncancelled required-checkpoint failure, only when both originals and pending inputs were saved. The transcript persists it as Assistant context before the terminal failure. The next ordinary request receives selected genuine tool observations with immutable JSONL source coordinates; later checkpoints retain these observations independently of the model-written summary. No pending job is executed automatically, and this is not a fallback semantic checkpoint or proof of tool success/current file state. Current user instructions retain precedence. Stop, document-preparation failures and storage failures keep existing archive recovery behavior.

The complete record, including task prefix, is bounded to one sixth of usable message tokens, clamped to 1,024–8,192 bytes. The task excerpt is at most 128 UTF-8 bytes. Action receipts take priority over retrieval excerpts; omitted observations remain in originals. Only Assistant-role internal blocks may contribute carried observations, so User-authored lookalike blocks cannot create completed-action receipts. Content belongs in the transcript, while diagnostics record only publication/context-replacement flags.

## Local evidence

This is local mock-provider and shared-server/CLI evidence, not live-model acceptance:

- `cargo test -p themis-desktop --test suspended_progress_e2e`: two passing tests. A real CLI sends requests through an isolated shared app server. A shell tool appends once, native reads create genuine context pressure, and all three summary attempts fail. After server shutdown/restart, the first ordinary continuation packet contains the append observation. Its coordinates dereference the exact archived tool result, and the file remains one line. A second case forces another genuine checkpoint whose valid model-written summary deliberately omits the append observation; the following ordinary request still has it. A later unrelated User request retains precedence. Successful runs do not publish the failure record.
- Core focused regressions pass for Unicode task-prefix pressure and rejection of User-authored completed-action blocks. Both initially failed, as did the missing first-continuation observation and later-checkpoint carry cases; the RED/GREEN logs are preserved externally.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed. Its first run found a missing exhaustive bridge-test event match; the match was fixed and the complete check rerun.
- `cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten`: 473 passed, zero failed, three ignored and the named credential-mutating test excluded. The ignored cases are the real public-import acceptance, live Go generation and boxed-tools doctest.
- `npm --prefix web run lint`: passed.
- `npm --prefix web test`: 310 tests across 30 files passed.
- `npm --prefix web run build`: passed, with the existing large-chunk warning.
- `ruff check quality_dashboard`: passed.
- `python3 -m unittest quality_dashboard.test_dashboard`: 14 passed.

The refreshed dashboard analyzed 116 product files with rust-code-analysis 0.0.25: mean cyclomatic complexity 2.55, cognitive complexity 2.17, maintainability 73.1. Runtime means are 2.78/2.68/66.9, versus 2.77/2.68/66.8 before this patch; inline Rust tests are included. Transcript is 2.14/0.67/82.4; desktop types 1.78/0.25/62.3; reducer 3.61/3.66/73.6. The existing 89.1% Rust coverage report is stale and does not establish changed-line coverage. Frontend coverage was not generated.

A reviewer agent cleared the current patch, tests and documentation after checking failure ordering, bounds, action provenance, latest-user precedence and independent carry through later compaction. Native installation/visibility proof remains pending; the next step is to build and stage this verified source without dispatching a model run.

## Live limitation and provider research

The last 200k narrative run failed all three LongCat summary requests with HTTP 429 and the generic endpoint-unavailable body. No semantic checkpoint was installed. The two waits totaled approximately 11 minutes 25 seconds; longer backoff did not resolve that failure. Existing originals, pending inputs and protected project files remain preserved. This patch does not backfill observation records into that earlier failed run.

Read-only authenticated OpenCode Console observations at 2026-10-11 11:59 UTC confirmed rolling usage 0%, weekly 100%, monthly 51%, with a weekly-limit banner and roughly 12 hours until reset. Allowed regions was Global. No provider request or account/privacy/billing change was made. This confirms a current account restriction, but does not prove how the earlier gateway errors were mapped or whether these specific free-model entitlements should be blocked.

[OpenCode Go documentation](https://docs.opencode.ai/docs/go/) recommends a distinct client user-agent and a stable per-conversation session header. Themis already sends these on ordinary and summary requests. The docs also list LongCat 2.5 Preview Free and Step 5 Preview Free as Unlimited. [Open issue #51682](https://github.com/anomalyco/opencode/issues/51682) reports account caps blocking free models; that report is supporting context, not proof of this account's historical error cause. [Maintainer guidance on endpoint unavailability](https://github.com/anomalyco/opencode/issues/42750#issuecomment-5941416618) distinguishes a past gateway outage and possible privacy-region restrictions.

Another long Go trial is deferred while the Console explicitly reports the weekly limit. No model/provider switch, paid-balance activation or additional increase in backoff was made. The complete single-chat live matrix at 200k, 256k and 24k remains incomplete. Local restart tests do not establish narrative accuracy, live semantic retention, autonomous multi-file coding or full-task efficiency.

External immutable receipts and full logs are under `/Users/cutedandelion/ThemisOS/Verification/compaction-20261010/adaptive-context/single-thread-v1/`, including `suspended-progress-local-gates/`, `opencode-go-console-limit-20261011.json` and `opencode-go-prevention-research-20261011.json`.
