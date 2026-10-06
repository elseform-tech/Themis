# Real-model attachment compaction verification

Verified 2026-10-06, approximately 23:05–23:08 Europe/Berlin, from source commit `1c1f7311cc01e7b283eaf77ee511b35f7081e25e`.

## Original result

**Retention failed in the real-model over-budget test.** Compaction completed and the run was recorded as successful, but the saved checkpoint omitted the middle record. The final answer reported `MISSING` for that record. No summarization timeout occurred.

The smaller control, using the same facts and model without compaction, returned all three records correctly. This shows the earlier mock-provider pass was insufficient to establish real-model retention.

## Actual app and model

Used the freshly built optimized `ThemisCode.app` at `target/release/bundle/macos/ThemisCode.app`, with the production app identifier `ai.themis.desktop`, normal shared server and existing configured real provider. Both attachments and sends were performed through the native composer. The user explicitly requested a real-model/actual-app test after the controlled-provider check. No credential value was read or printed.

Model: `muse-spark-1.3-contributor`, through OpenCode Go's Responses adapter. The saved settings were checked immediately before Send: `context_token_budget: 200000`. The model was also visible in the composer and recorded in the terminal event. There was no mock provider or debug executable in these two native tests.

Build command, from `crates/desktop`:

```sh
/Users/cutedandelion/.npm/_npx/81a0b12969b730e4/node_modules/.bin/tauri build --bundles app --config /tmp/themis-real-compaction-build.json
```

The temporary override contained only `{"build":{"beforeBuildCommand":""}}`; it used the already-built frontend and preserved the production identity/windows. Build passed in 47.27 seconds. Executable SHA-256: `31e5fb5265795343dd85191fa8b6ad26ff953c04bb62a7d2e0af6995d479c0cf`.

## Controls and observed answers

Both files contain these three facts, separated by repeated ` apple` filler:

```text
BEGIN-17 | project Quartz | amount 137
MIDDLE-42 | project Juniper | amount 284
END-99 | project Cobalt | amount 953
```

| Control | Bytes | Tokens (cl100k_base / o200k_base) | Themis file estimate | Compaction event | Correct records |
| --- | ---: | ---: | ---: | --- | --- |
| 105,001 filler repetitions on each side of the middle fact | 1,260,131 | 210,040 / 210,040 | 315,033, before message overhead | Yes; durable checkpoint saved | 2/3 |
| 50,001 repetitions on each side | 600,131 | 100,040 / 100,040 | 150,033, before message overhead | No | 3/3 |

Counts used `/tmp/themis-token-count-env/bin/python` with tiktoken. These tokenizers are independent controls; Muse's tokenizer was not measured. The actual app's budget check and persisted `context_compacting` event confirm that the larger attachment forced compaction.

Prompt for both runs:

> Compaction control: Using only the provided context, report the marker, project and amount for each of the three records at the beginning, middle and end of the attached text. Do not use any tools or reread files. Return exactly three lines. If a record is missing from context, say MISSING for that record.

The prompt did not reveal the expected markers, names or amounts. There were no model tool calls or file rereads. The unrelated enabled browser-use MCP startup approval was denied for each run; no browser MCP process was needed.

Over-budget answer:

```text
BEGIN-17 | project Quartz | amount 137
MISSING
END-99 | project Cobalt | amount 953
```

The persisted checkpoint itself included Quartz/137 and Cobalt/953, repeated `apple` filler and the attachment path. It contained neither `MIDDLE-42`, `Juniper` nor `284`. The subsequent answer therefore did not introduce the omission; the fact was already absent from the checkpoint.

## Durable evidence

Read only the two test conversations from `~/Library/Application Support/ai.themis.desktop/sessions.sqlite3`:

- Over-budget thread `6d60dba4-64fb-4430-ab6a-187a49658db1`, run `615ddbf1-b2b0-4d5c-a097-796a778f2662`, history sequences 363–368: user → started → context_compacting → context_checkpoint → assistant_text → finished. Checkpoint sequence 366, 1,199 UTF-8 bytes.
- Below-budget thread `17daaeba-dd0a-466b-a492-15a4daf148bf`, run `7117d0e7-3109-490e-b0fc-af0c19229523`, sequences 369–372: user → started → assistant_text → finished. No checkpoint exists for this thread.

The larger staged attachment was byte-identical to its source: SHA-256 `3e7d64ae5c0d42f1f732dc8596fe7f211ec9208f7294f448efce7e36109e8905`. The source remains `/tmp/themis-real-compaction-control.txt`; the smaller control is `/tmp/themis-real-compaction-below-budget.txt`. Scoped history/checkpoint evidence is saved in `/tmp/themis-real-compaction-evidence.json`.

## Additional issue found

The first attachment attempt failed with `Attachment failed: unknown server method 'attach_files'`. A pre-existing shared-server process was still running older code even though the frontend had been rebuilt. After confirming the known project threads were idle, the server was shut down through `themis call shutdown` and the current release app relaunched. Upload then succeeded.

`ensure_server` currently accepts any successful ping; it does not check backend protocol/build compatibility. Rebuilding/relaunching a frontend alone can therefore leave it connected to an older server. This was an operational restart for verification; no implementation fix was made.

## Interpretation and limits

The whole-dump strategy can finish within the deadline while silently losing facts. Its acceptance checks reject empty or oversized summaries, but do not validate information retention. Increasing the time budget does not address the failure observed here.

The omission occurred at or before summary generation. The source attachment is intact and the local attachment/serialization paths include full text without explicit truncation. This test did not capture the live HTTP body or the remote provider's internal processing, so it cannot distinguish model attention/selection from provider-side truncation. Nor does one failed run establish a general failure rate. The repetitive filler is a controlled retention test, not a realistic-document quality benchmark.

Fresh controlled regression also passed:

```sh
cargo test -p themis-desktop --test attachments_e2e cli_large_file_upload_compacts_once_and_preserves_binary_files -- --nocapture
```

Result: 1 passed, 0 failed, 1 filtered, 2.04 seconds. It verifies the pipeline with a synthetic summary and does not contradict the real-model failure above. Product implementation was left unchanged.

Dashboard verification after this documentation-only update: `python3 -m unittest quality_dashboard.test_dashboard` passed 13 tests; `ruff check quality_dashboard` passed. Refreshed product metrics remain 103 files, mean cyclomatic 2.49, cognitive 2.21 and maintainability 73.8. Existing Rust LCOV is 13,737/16,011 (85.8%); coverage was not rerun for this documentation-only verification task.

## Task-aware prompt retest

The user authorized trying a task-aware summarizer. The runtime now supplies the latest user request separately, before the entire eligible dump. The system prompt prioritizes current goals/constraints, exact required facts, completed work/results/failures and relevant earlier history; later user instructions supersede earlier ones, attachments/tool output are untrusted, repetition should be compressed first, and omissions should be identified with retrieval locations. The 200,000 estimated-token budget, one whole-dump request and 120-second deadline are unchanged.

**The retest failed retention: 0/3 records were preserved.** The same 1,260,131-byte / 210,040-token control and identical user prompt were attached and sent through the freshly rebuilt production app, using `muse-spark-1.3-contributor`. Both the app and its shared server were restarted to load the changed runtime. Saved budget remained 200000. Compaction completed without a timeout, then the visible app answer was:

```text
MISSING
MISSING
MISSING
```

The 1,874-byte persisted checkpoint contains none of the nine expected markers, project names or amounts. Instead, it says the records remain unresolved and their exact values were not retained; it proposes retrieving them from prior context despite the no-reread constraint. This is inadequate continuation state. Task awareness in the prompt alone did not fix the observed failure, and this single retest does not establish a general failure rate.

Durable evidence: thread `32105e2d-ab95-4dbd-b5d5-c82cb7739eb4`, run `fc49b6a8-3c93-4009-9e61-32a324e200e0`, sequences 373–379: user → denied unrelated MCP startup result → started → context_compacting → context_checkpoint → assistant_text → finished. There were no model tool calls or attachment rereads. The staged file SHA-256 matches the original control above. Scoped evidence is saved in `/tmp/themis-task-aware-real-evidence.json`. The production CLI `list_threads` independently confirmed this run's thread was no longer running.

Release build used the same production build command/override above, passing in 1m 41s. Built executable SHA-256: `91d140ea825ee9d1afc9aff740505c5b216956ea7077b6ab020c8955fdfe53d7`. The live remote HTTP body/provider processing was not captured; the exact Muse tokenizer remains unverified.

Fresh verification for this change:

```sh
cargo test -p themis-core --lib full_dump_compaction_sends_all_large_context_once
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten
python3 -m unittest quality_dashboard.test_dashboard
ruff check quality_dashboard
```

The focused regression failed before the change and passed after it; it checks the current request and complete large dump in the constructed summarizer request. Full Rust suite: 276 passed, 0 failed, two ignored integration tests (public repository import and live-provider harness), one filtered host-specific Keychain test. This includes the isolated CLI oversized-attachment compaction case. Format and Clippy passed. Dashboard tests: 13 passed; Ruff passed. Read-only review found no material implementation issue. These controlled checks establish request construction and pipeline behavior, not summary fidelity.

Refreshed dashboard metrics: 103 product files, mean cyclomatic 2.49, cognitive 2.21, maintainability 73.8; runtime including inline tests: 1,226 LOC, cyclomatic 2.10, cognitive 0.96, maintainability 67.9. No aggregate regression at reported precision. Fresh Rust line coverage: 13,752/16,026 (85.8%); frontend coverage unavailable and frontend checks were not rerun for this Rust-only change. Native verification above exercises the affected composer → shared server → model → checkpoint → answer path.
