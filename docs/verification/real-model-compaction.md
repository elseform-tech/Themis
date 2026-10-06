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


## Root-cause investigation and bounded retention improvement

The whole-dump input was intact locally. A temporary, synthetic-control-only request capture recorded the production summarizer HTTP body: 1,261,993 bytes, SHA-256 `360c8cbca5961de6bd9da080925d91cee9be9984b005c9ad7e28e94209be13c7`. Its user message contained all three records; the parsed attachment content ended with the complete, byte-identical 1,260,131-byte source file. The request was not clipped at the configured 200,000 estimate. Capture instrumentation was removed before the final build; authorization headers and credentials were not captured.

That capture run (thread `5d3049e4-ca54-44c2-bed3-67741c12c35a`, run `7f849b85-46c5-4781-acbe-3ff80bae9312`, sequences 380–384) hit the existing 120-second summary timeout. Replaying the captured JSON body against the same configured Responses endpoint/model completed in 13.48 seconds and reported **210,441 input tokens**. It retained the project/amount values but corrupted the middle and end markers into `BEGIN-42` and `BEGIN-99`. This distinguishes the timeout from completed-but-inaccurate summaries. The replay used a different test session header and user agent; it was an identical JSON body, not an identical complete HTTP exchange.

An extraction-first instruction with the relevance request and final summary instruction placed after the dump completed a replay in 16.00 seconds, reported 210,521 input tokens and retained all three records correctly. Whole-input direct probes also succeeded with that prompt placement, but the actual app retest with changed records still omitted the middle record. Some repeated probes used cached input, so they are not independent uncached trials.

The supported root cause is factual loss or corruption during remote summary generation, sensitive to prompt placement and input size. The checkpoint already lacked the facts before the continuing answer was generated. The provider's internals remain unobservable: reported input usage is evidence that the request was accepted and billed at that size, not proof of how every token was processed internally. Neither a larger output allowance nor a longer timeout alone addresses a successfully completed inaccurate summary. Existing checkpoint acceptance checks verify nonempty text and resulting context size, not semantic retention.

### Controlled comparison

All native runs used `muse-spark-1.3-contributor`, a saved context budget of 200000, and the same no-tools/no-reread control prompt above. The prompt never revealed the expected values.

| Strategy / evidence | Retention result |
| --- | --- |
| Original whole dump, native run | 2/3 records; middle missing |
| Task-aware instruction before whole dump, native run | 0/3 records |
| Extraction-first instruction and task footer, whole dump, native original | 3/3 records |
| Same improved whole dump, native changed records | 2/3 records; middle missing |
| 128,000-byte sections, changed middle record at 25%, 50%, 75% in direct probes | 2/3 positions; 75% missing |
| 64,000-byte sections, same three positions in direct probes | 3/3 positions |

The successful whole-dump native run was thread `89c50696-0256-4fa5-beca-d4a171f82015`, run `f660caf1-0280-4179-98d1-e69a8c4d1f84`, sequences 385–391. Its changed-record counterpart was thread `40e9afce-2071-43e1-adec-0eee87c2bd9b`, run `cd2e792f-2ad2-40ee-987e-d94e01630812`, sequences 392–398. Both checkpoints were inspected, and neither model used tools to recover omitted records. The changed fixture is 1,260,132 bytes, 210,039 `cl100k_base` tokens / 210,038 `o200k_base` tokens, SHA-256 `efd87c44aa224592d37647d6ed8376ba146b5762988b675d5bcae21ba0ef5280`; neither tokenizer is claimed to be Muse's tokenizer.

The best tested improvement therefore combines extraction-first instructions with bounded input, rather than relying on a successful whole-dump trial. The shared runtime now sends every eligible dump byte through sections of at most 64,000 UTF-8 bytes with approximately 1,024 bytes of overlap, runs up to three section requests concurrently, and concatenates their summaries in source order. There is no second summarization pass that could discard extracted records. Partial sections explicitly cannot establish global absence. Each relevance hint is bounded to first/last excerpts when the latest user request exceeds 8,192 bytes; the complete original active request remains in continuing context. These byte bounds are a tested heuristic, not a tokenizer-based capacity guarantee or a fact-specific parser.

The configured context budget remains 200,000 estimated tokens and the global compaction timeout remains 120 seconds. Any empty section, provider error, cancellation, timeout or oversized resulting checkpoint leaves original context intact. Larger histories can still exceed the time limit, and dense facts can produce an oversized checkpoint. The implementation does not silently drop sections to fit. The overlap protects short records crossing boundaries; it does not guarantee preservation of arbitrarily long dependencies. Arbitrary documents and future repeated compactions still need broader retention evaluation.


### Fresh verification of the bounded implementation

```sh
cargo test -p themis-core --lib bounded_compaction_sends_all_large_context
cargo test -p themis-core --lib context_sections_cover_unicode_and_boundary_records
cargo test -p themis-desktop --test attachments_e2e cli_large_file_upload_compacts_once_and_preserves_binary_files -- --nocapture
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten
python3 -m unittest quality_dashboard.test_dashboard
ruff check quality_dashboard
```

Focused tests passed. The large-context regression failed before sectioning, and its oversized-active-request case failed before bounding the relevance hint. It now checks bounded outgoing requests, delivery of all three distributed records and preservation of the complete active request. The Unicode/boundary regression checks byte coverage, valid UTF-8 slicing and a record crossing a section edge. The isolated shared-server CLI case checks section requests, a single atomic checkpoint, final answer and binary attachment paths. These mocked checks establish the pipeline, not model fidelity.

Full Rust coverage run: **277 passed, 0 failed, two ignored integration tests, one filtered host-specific Keychain test**. Complete output was read. Format and Clippy passed. Dashboard tests: **13 passed**; Ruff passed. Review found no unresolved material implementation issue after fixing the oversized relevance hint. No dependencies were added.

Refreshed dashboard metrics: 103 product files, mean cyclomatic 2.49, cognitive 2.22, maintainability 73.8. Runtime including inline tests: 1,309 LOC, 46 functions, cyclomatic 2.19, cognitive 1.14, maintainability 67.3. Compared with the previous whole-dump version, runtime complexity rose and maintainability fell by 0.6 as sectioning and UTF-8/request bounds were added; aggregate cognitive rose 0.01. Fresh Rust LCOV: **13,825/16,104 (85.8%)**. Frontend coverage remains unavailable; frontend checks were not rerun for this Rust-only change. Native verification covers the affected composer/model/checkpoint path separately.

Production release build passed in 1m 59s using the production command/override described above. Final executable SHA-256: `65318aa790492ea3e3196973c7b3963ee0d8487271e2b843d020825180c34f29`. The app and shared server were restarted before verification so both used this runtime. Saved budget was rechecked as 200000.


### Final production-app retention controls

**Both bounded-section controls retained 3/3 exact records in their persisted checkpoint and final three-line answer.** Both fixtures exceeded 200,000 tokens under the reference tokenizers, forced compaction at the unchanged 200000 setting, used the configured real model, and completed within the unchanged global timeout. Neither continuing model used tools or reread the attachment. These are two controlled runs, not a general success-rate estimate.

- Original fixture: thread `d6a571cc-c86a-42cd-a6c0-8285003b9b7f`, run `35e5b183-d93b-4b70-9096-e0e16b0e40ee`, sequences 399–404. Sent through the production CLI/shared app server and final answer observed in the native app. Checkpoint: 40,514 UTF-8 bytes, all three exact records retained; final answer exactly matches the original three records above. Staged file SHA-256 matches its original source.
- Changed fixture: thread `d3926fdd-6a7c-4732-b29e-38ca9c959801`, run `55702ace-5676-4ef0-baa7-64958d91e5c2`, sequences 405–411. Attached and sent through the native composer; visible “Compacting context…” transitioned to the final answer below. Checkpoint: 39,894 UTF-8 bytes, all three exact records retained. The unrelated browser-use MCP startup was denied (sequence 406); it was not a model recovery tool call. Staged attachment SHA-256 matches the changed source above.

```text
START-63 | project Maple | amount 418
CENTER-28 | project Willow | amount 672
FINISH-81 | project Indigo | amount 305
```

Scoped local evidence: `/tmp/themis-bounded-original-evidence.json` and `/tmp/themis-bounded-variant-evidence.json`; authoritative events/checkpoints remain in the production SQLite store. Direct probe logs and capture/replay artifacts are temporary investigation files and are not committed. The final native app remains open on the changed-fixture answer.

An additional observation: reopening the UI during an active server run temporarily displayed “Run interrupted” while the server still reported the run as active; the final event corrected the view. Active-run hydration was not changed as part of this compaction fix. This and the stale-server compatibility issue recorded above are separate remaining UI/operational issues.
