# Recoverable compaction verification — active goal

Status: implementation and verification ongoing; do not claim completion or lossless summary retention.

Goal: compact working state while durably preserving original conversation/tool/source evidence and validating retrieval after repeated real-model compactions and restart.

Implementation remains on `codex/full-dump-attachments`, continuing the existing compaction task. Production budget200000, global checkpoint deadline150s; section input64000UTF-8bytes, overlap1024, concurrency3. New summaries aim for300words; evidence references are deterministic. Source-complete recovery checkpoints explicitly mark unavailable summaries. Original-source availability and summary-only retention must be measured separately.

Acceptance: fresh workspace/CLI checks, fault/cancellation/budget/storage checks, repeated narrative runs using the unchanged six source files and 24 preregistered criteria, recovery-assisted source checks after production server restart, and a coding task with large repository/tool context, source-version recovery, requirements/unrelated changes preserved, and fresh executable tests. Report all failures and limitations.

Preliminary production build SHA2564268e296ec65ba3e59fc5da33b3aeab950e5343260d6716e7ba9f190ff974243 (before storage/fallback fixes). Thread55a3beb8-1f75-4e8c-a975-c463c816d081: part1 checkpoint35278bytes, finished~90s; part2 checkpoint41848bytes, finished~150s (includes continuing answer); part3 timed out at150s. No final retelling or24-fact score. Saved originals for all three parts remain available. No semantic reliability claim follows from this partial run.

Local artifacts: `/tmp/themis-narrative-six/run-evidence.py`, `run-evidence.log`, `thread-evidence.json`, `evidence-recoverable.json`; `/tmp/themis-evidence-server.log`. Earlier fixtures/hashes/rubric unchanged under `/tmp/themis-narrative-six/`. Unit red/green logs `/tmp/themis-evidence-*.log`. Final production retest and coding journey remain pending.

## Implementation and fault checks

Committed implementation: `9a846e2 feat: preserve recoverable evidence across compaction`. Final production binary SHA256 `de9ccee2c869aba6917f9d3b2a7a14a81d16fae6d0ad644c504d289ed5940f7e`. Both the production native window and its shared server were restarted with this bundle. A first runner launch raced server startup and failed before creating a thread; the retry created thread `893b1365-f50d-44e6-8946-4b0d7883308d`.

Fresh checks:

- `cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten`: 283 passed, three ignored (public repository import, live provider unit harness, tool documentation example), one known synthetic Keychain test excluded.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all -- --check`: passed.
- `cargo test -p themis-core --lib`: 97 passed; focused timeout, provider failure, empty/oversized summary, cancellation, atomic publication and blocked checkpoint queue cases passed.
- `cargo test -p themis-core --test runtime_e2e`: nine passed, live-provider case ignored. Full tool results and evidence references survive two checkpoints.
- `cargo test -p themis-desktop --test attachments_e2e`: two passed. Production server/CLI restart and an actual approved Python shell command recovered a fact deliberately omitted by the mock summary; original bytes remained unchanged.
- `python3 -m unittest quality_dashboard.test_dashboard`: 13 passed; `ruff check quality_dashboard`: passed.
- `cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten`: passed; Rust line coverage 14,386/16,660 (86.4%). Frontend coverage unavailable. Frontend source was unchanged and its UI journey was not retested in this backend task.

Dashboard `/api/metrics` refreshed at `2026-10-07T14:52:03Z`. rust-code-analysis 0.0.25 analyzed 103 files: mean cyclomatic2.49, cognitive2.22, maintainability73.7. Previous values2.49/2.22/73.8. Runtime including inline tests:1769LOC,55functions, cyclomatic2.30,cognitive1.34,maintainability67.8; previous1392LOC,47functions,2.15/1.14/67.5. Runtime complexity averages increased modestly with archival/failure branches and regressions; these include inline tests and do not isolate production complexity. Attachments module219LOC,2.50/0.81/78.7; run module389LOC,3.47/4.00/80.3. No unrelated metric cleanup was made.

Final narrative source audit currently confirms complete original text for the first three parts, with unchanged fixture hashes; final six-part and semantic results remain pending. Local logs: `/tmp/themis-evidence-workspace-final.log`, `themis-evidence-clippy-final.log`, `themis-evidence-coverage-final.log`, `themis-evidence-metrics-final.json`, `themis-evidence-server-final.log`; narrative `run-evidence-final.log`, `thread-evidence-final.json`, `original-audit-final.json`. Coding fixture is prepared separately under `/tmp/themis-coding-evidence/` and `~/ThemisOS/Projects/Compaction Coding Evidence`: actual tracked ThemisOS reference source (1,622,006bytes) embedded in three controlled historical observations, an executable Python quote regression and a pre-existing unrelated modified file. It has not yet been run through the model.
