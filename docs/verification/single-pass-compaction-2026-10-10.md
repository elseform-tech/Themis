# Single-pass compaction verification — 2026-10-10

Status: latest 24k coding artifacts pass independent checks, but autonomous continuation is not yet accepted. Recent-observation and cold-catalog fixes are rebuilt; the next live retest awaits Mac/Keychain unlock. Latest build: `082caf6`. Both latest-build narrative budgets and 200k coding remain unverified. Original implementation: `bc8ef3a` on `codex/bounded-tool-context`, following `47aabfd`.

## Implementation

Compaction now saves complete originals, prepares one bounded input locally, then asks the provider for one continuation state. It may retry once for a transient failure, truncated output or empty text. No section fan-out, recursive subdivision or merge request remains. This adopts the simpler single-summary shape discussed for OpenCode; it does not import OpenCode code or claim identical behavior.

Preparation preserves the prior task state, current request hint, source roles/order, invocation/result IDs and original JSONL record coordinates. It removes adjacent identical Assistant narration and marked recovery copies. Oversized fields use UTF-8-safe head/tail excerpts with explicit omitted decoded-byte ranges; tool argument/result fields additionally select at most 8,000 source bytes. The actual serialized packet must fit the message allowance after reserving twice the initial summary output plus 1,024 estimated tokens. Sampling without a saved original is rejected before any provider request. Metadata and prior state that cannot fit trigger the existing explicit fallback instead of another summarization pass.

Original-first recovery, durable tool journals, action receipts, bounded read/location caches, permissions, provider deadlines, Stop, atomic saves and fitting-context replacement remain shared safeguards. An independent runtime coverage notice makes omissions visible even if model text overlooks them. Summary failure keeps the previous good task state. Consecutive exact unchanged call/results now stop after three duplicates rather than running to the normal turn limit; changed results or other actions reset the counter. Commands still execute. The existing archive-recovery cap remains separate. These limits bound work; they do not guarantee semantic retention or prevent every possible loop.

`compaction.summary_max_tokens` defaults to 8,192 and keeps the existing dynamic caps. `section_max_tokens` remains accepted for compatibility but no longer affects execution. No new dependency, registry or configuration surface was added.

## Build and audit protocol

`cargo build -p themis-desktop --bins --features tauri/custom-protocol` and the frontend production build passed. The development bundle was replaced atomically, ad-hoc signed and verified with `codesign --verify --deep --strict`. All 69 previously known persisted chats were checked idle before relaunch. Unrelated QA backends were preserved. New normal frontend/backend PIDs were confirmed from the bundle path.

Bundle: `/Users/cutedandelion/Documents/ChatGPT/ThemisOS/target/debug/bundle/macos/ThemisCode.app`. Binary SHA-256: `8c50f0d3e174de9fab712c1dfd65e1771f932075ed734a0937540db0826a1c93`. This is a development custom-protocol build, not a signed release package. Runtime source hash at launch: `18284ab4b60fc311765762b769a86b2ba68bac8eb7ab1ed77980c81f6a148c2b`.

Evidence root: `/tmp/themis-single-pass-20261010/`. Protocol was frozen before live requests in `protocol.json`. OpenCode Go `longcat-2.5-preview-free`, default reasoning, project-only 24,000 and 200,000 estimated-token budgets, two recent messages and 100 tool turns. At most two live chats run concurrently. No corrective operator prompts or success-selecting retries. Observer limits: coding 25 minutes, each initial narrative part 12 minutes, separate recovery 20 minutes. Elapsed times are observations, not controlled latency comparisons.

Coding uses eight attachments (1,623,145 bytes), six broken modules and four immutable reference tables. Both fresh baselines fail the 16 supplied tests with 20 subtest failures and four errors. Acceptance requires natural completion, those tests, 210 independent checks, unchanged requirements/tests/staged originals, standard-library-only imports and byte-identical reference blocks.

Narrative uses six consecutive renamed Clarissa parts (5,042,808 bytes), each large enough to force compaction at both budgets. Initial continuity forbids tools/rereads: five exact acknowledgements, then a chronological retelling, 24 questions and four exact short quotations. Recovery is a separate phase permitting bounded local reads and requiring source anchors. Acknowledgement is not retention evidence. The frozen rubric scores 24 facts, ten chronology pairs and four whitespace-normalized quotes; technical failures and observer stops remain unscored. Recognizable literature and assistant-audited rubrics limit generalization. This is an audit, not an automatic runtime gate.

## Automated checks

- `cargo fmt --all -- --check`, `git diff --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test -p themis-core --lib`: 140 passed.
- `cargo test -p themis-core --test runtime_e2e`: 19 passed, one opt-in live test ignored.
- `cargo test -p themis-desktop --test attachments_e2e`: seven passed. The full coverage run repeats them against the final test source. It exercises the actual constructed system message, Python recovery recipe and CLI/provider path, including recovering an omitted middle passage after summary failure and restart.
- `cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten --test-threads=1`: 366 passed, two opt-in tests ignored, one host Keychain test explicitly excluded. Rust line coverage: 20,387/22,944 (88.9%). Frontend line coverage is unavailable; this does not establish Keychain persistence.
- `npm --prefix web run lint`, `npm --prefix web test`, `npm --prefix web run build`: passed; 306 tests in 29 files. Existing Vitest deprecation/localStorage and Vite chunk-size warnings remain.
- `ruff check quality_dashboard`, `python3 -m unittest quality_dashboard.test_dashboard`: passed; 14 dashboard tests, separate from product tests.

Before the replacement, the new 24k request-bound regression observed 44 provider requests for one checkpoint. The final regression allows one successful request or at most two attempts at both budgets, rejects empty output, checks UTF-8 and preserved originals, and verifies cancellation/deadline behavior. Additional red-before-fix cases cover repeated A→B→A user corrections, invocation/result pairing, alternating ordinary reads and short records whose omission markers would be larger than their text. Read-only review findings were fixed and re-reviewed with no remaining findings. Review is separate from executed tests.

Early attachment/full-coverage attempts failed on obsolete assertions expecting section-note artifacts. Those failures were retained in the work history, the assertions were migrated to the prepared-packet behavior, and the fresh final coverage run passed. They are not counted as successful attempts.

## Code health

The dashboard refresh analyzes 107 product files with rust-code-analysis 0.0.25. Product mean cyclomatic/cognitive/maintainability: 2.56 / 2.24 / 73.4. Runtime: 4,324 LOC, 2.67 / 2.49 / 68.1. The prior committed audit reported runtime 4,808 LOC, 2.58 / 2.30 / 68.3 and Rust coverage 89.2%. Retiring the section scheduler reduces LOC but average callable complexity slightly increases and coverage is now 88.9%; this is not a claim of universal metric improvement. Inline Rust tests are included in LOC/AST; the denominator changes with deleted code/tests. The large runtime remains a maintenance concentration. No speculative refactor was added merely to change these scores.

## Live results

### Large multi-file coding

| Budget | Outcome | Seconds | Tools / compactions | Independent artifact checks |
| --- | --- | --- | --- | --- |
| 24k | Provider HTTP 500: malformed function-call JSON (invalid escape); no final answer | 832.13 | 37 / 6 | 16 supplied tests fail: three failures, 17 errors; holdout aborts on first randomized ledger |
| 200k | Provider stream timeout; no final answer | 301.23 | 9 / 1 | Baseline unchanged: 20 failures, four errors; holdout aborts on first randomized ledger |

At 24k, five modules were changed through local Python scripts. `parser.py` uses `re.fullmatch` without importing `re`, and the CLI remains unrepaired. The malformed recovery call failed before execution. At 200k no source was edited before the stream timed out. Neither run passes autonomous coding acceptance. The remaining holdout checks were not executed after the first error; do not report a 210-check pass or an invented total score.

Both runs preserve all eight staged/full JSONL attachment originals, requirements/tests, standard-library-only imports and all four reference blocks byte-for-byte. Previously captured snapshot hashes are unchanged: 43 at 24k and ten at 200k. These preservation results do not imply correct edits.

Commands: `python3 -m unittest discover -s tests -v` in each project; `python3 /tmp/themis-single-pass-20261010/coding/24k-oracle.py` and `200k-oracle.py`; `python3 /tmp/themis-single-pass-20261010/coding/verify_preservation.py 24k` and `200k`. Full outputs are retained as `*-tests.log`, `*-oracle.log`, `*-preservation.json`.

### Narrative continuity and recovery

The 24k recovery phase ended after 894.43 seconds at the existing retrieval guard, with no final answer.

| Budget / phase | Outcome | Tools / compactions | Semantic scores |
| --- | --- | --- | --- |
| 24k initial | Provider stream timeout; final turn 372.36 s, all six turns 1,064.52 s | 2 / 6 | Unscored: no final answer; forbidden archive calls separately fail strict continuity |
| 200k initial | Provider HTTP 500 malformed function-call JSON; final turn 364.23 s, all six turns 863.93 s | 10 / 6 | Unscored: no final answer; forbidden rereads separately fail strict continuity |
| 24k recovery | Retrieval guard; no final answer | 53 / 7 additional | Unscored: no final answer |
| 200k recovery | Provider HTTP 500 malformed function-call JSON after 93.28 s | 16 / 0 additional | Unscored: no final answer |

At both budgets all five acknowledgements were exactly `RECEIVED`, but the final retellings never arrived. At 24k three compactions succeeded and three fell back; all six complete originals, source/staged hashes and eight previously captured snapshots pass preservation checks. The two forbidden 24k archive calls returned Assistant navigation and a no-match, rather than a newly retrieved story passage. They still violate the no-tool protocol. At 200k two initial compactions succeeded and four fell back. The separate recovery read local source/staged files but failed before a final retelling. All six complete originals remain in JSONL; source/staged hashes and 16 previously captured snapshots are unchanged. Verification: `python3 /tmp/themis-single-pass-20261010/narrative/verify.py 200k initial` and `recovery`. The score files use null for technical failures; false quote-presence booleans in the hash/extraction audit are not zero semantic scores.

Both initial runs violated the no-tool instruction on part four: 24k made two archive reads; 200k made a bounded `read_file` call and additional source reads on part six. Its checkpoint still explicitly contained “Do not call tools, reread files, search archives, or use external knowledge.” This is an observed instruction-following violation, not proof that compaction dropped the constraint. Neither run can count as a strict no-reread continuity pass; final semantic scores must disclose those tool calls. The audit uses the same YOLO mode as the prior protocol: tool prohibition is a user instruction, not a mechanically disabled tool set. A future isolated no-reread audit should also deny source access through runtime policy; this trial is reported as actually run.

## Shell-history investigation

The 24k coding and 200k narrative failures exposed a shared harness bug. The runtime correctly rejected invalid JSON before execution, but the provider adapter replayed the same malformed arguments in subsequent structured tool history. Upstream Go then reported a matching invalid-escape location while parsing that history. The Messages adapter also parsed malformed historical arguments locally and could fail before making a request. These are distinct from Python SyntaxError, NotADirectoryError, failed assertions/tests, and provider timeouts.

The fix changes only outgoing wire history: malformed/non-object arguments become a valid object containing `_themis_rejected_tool_arguments`, with the raw value retained as a string. Tool IDs/results stay paired, valid object arguments stay unchanged, and saved originals remain exact. It never repairs or executes rejected commands. A model still must issue a corrected call through validation and approvals.

Regression: the protocol test failed before the fix on the live-shaped invalid escape, then passed for Chat Completions, Responses and Messages. The shared-server CLI regression forces compaction at both budgets, rejects a command that would create `unsafe.txt`, verifies it did not execute, executes a corrected Python script with apostrophe/backslash content, and confirms the malformed arguments remain in original snapshots.

Fresh live trials of the rebuilt fix are recorded separately under `/tmp/themis-shell-history-20261010`; they do not replace or rescore the first trial above.

## Rebuilt shell-history fix

Commit `cebf333a499824097b1a746a04cb17c88f8b5193`. Build command: `cargo build -p themis-desktop --bins --features tauri/custom-protocol`. Running development bundle: `target/debug/bundle/macos/ThemisCode.app`; binary SHA-256 `3d4c59ae3e6834685f4c8d4401f1be977f4430b09187b7d426c9240ecd6e154d`. Ad-hoc codesign verification passed. All known chats were checked idle before replacing the normal frontend/backend; unrelated QA servers and dashboard were preserved.

Fresh verification after the fix: `cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten --test-threads=1` passed **368 tests**, with two opt-in tests ignored and one host synthetic-Keychain test excluded. Rust line coverage is **20,540/23,088 = 89.0%**. `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`, `git diff --check`, `ruff check quality_dashboard` and `python3 -m unittest quality_dashboard.test_dashboard` passed (14 dashboard tests, separate tooling inventory). Full logs: `/tmp/themis-shell-{coverage,clippy,build,dashboard-tests}.log`.

Dashboard refreshed from `http://127.0.0.1:4178/api/metrics`, saved in `/tmp/themis-shell-history-20261010/metrics-verified.json`. Product averages remain 2.56 cyclomatic / 2.24 cognitive / 73.4 maintainability across 107 files. `providers/wire.rs`: 342 LOC, 4.71 / 3.78 / 67.6, versus 323 LOC, 5.05 / 4.00 / 66.9 before this fix. `providers.rs`: 2,161 LOC, 2.18 / 1.17 / 67.9 versus 2,091 LOC, 2.15 / 1.09 / 68.2; its regression test increases average complexity slightly. Inline Rust tests remain included in LOC/AST. Runtime metrics are unchanged. Frontend line coverage remains unavailable.

The first trial completed 26 checkpoints with 49 total summary attempts: at most two per checkpoint, no section/merge requests; 12 succeeded and 14 used fallback. Maximum observed checkpoint duration was 140.484 seconds. All first-trial temporary project overrides were backed up and removed after idle. Native inspection confirmed six saved story attachments and the final retrieval-guard failure.

The second trial retained the same model, fixtures, two-chat concurrency and predeclared observer limits. Its 200k coding case ended after 337.42 seconds with a provider stream timeout, 13 tool calls and one fallback checkpoint; no source edits were made. The supplied tests retained 20 failures and four errors, and the holdout aborted on its first case. All eight full originals, 14 captured snapshots, requirements/tests and four reference tables passed preservation checks.

The user then requested one test at a time, stopping to patch when an issue appears. The unfinished 24k coding and 200k narrative cases were stopped and remain unscored, with their partial histories preserved. The 24k narrative case was never dispatched. All four temporary project configurations were backed up and removed after the chats became idle. `superseded-by-timeout-change.json` records this intervention; these cases are not counted as completed acceptance trials.

## Five-minute deadline fix and sequential retest

Commit `da0e5085e917249f2fbec872a78f4821aaf48e2e` raises each provider HTTP request from 120 to 300 seconds and the default/maximum shared compaction deadline from 240 to 300 seconds. Both summary attempts still share that one deadline; ordinary answers can retry once before public text and therefore exceed five minutes per turn. Shell child-process timeouts remain separate. Summary output caps are unchanged: reducing them may increase truncated summaries rather than solve stalled streams.

The default/schema/boundary regression failed before the change (240 versus 300), then passed. `cargo test -p themis-core --lib`: 142 passed. The actual shared-server CLI shell-history regression passes at both budgets with explicit 300-second project configuration and resolver readback. Full `cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten --test-threads=1`: **369 passed, two opt-in tests ignored, one synthetic Keychain test excluded**. Rust coverage: **20,563/23,109 = 89.0%**. Clippy, rustfmt, diff check, Ruff and all 14 dashboard tests passed. Logs: `/tmp/themis-five-minute-{core,cli,coverage,clippy,build,dashboard-tests}.log`.

The development custom-protocol build was relaunched after checking all known chats idle, preserving unrelated QA servers. Binary SHA-256: `ce3b6db81f4981699e7eacbf51dd639f2f6d1d67c5041b62d081c148a393505a`; ad-hoc codesign verification passed. Backend configuration confirms the 300-second default. Evidence: `/tmp/themis-five-minute-20261010/build.json`.

The dashboard refresh retains 107 analyzed product files and product mean cyclomatic/cognitive/maintainability of 2.56/2.24/73.4. Configuration: 706 LOC, 2.84/2.78/65.3; providers: 2,161 LOC, 2.18/1.17/67.9; runtime: 4,324 LOC, 2.67/2.49/68.1. Inline Rust tests remain included; frontend line coverage is unavailable. Evidence: `/tmp/themis-five-minute-20261010/metrics-verified.json`.

Only the fresh 200k coding case is dispatched, using the same byte-identical broken baseline and eight attachments. The observer stops on harness failure and no follow-on cases dispatch automatically. Live results are pending. Native inspection after this relaunch is currently blocked by the locked Mac; CLI verification continues.



## Credential-read freeze and sequential 200k trial

Commit `97910f0e5daaaf896057f2e1cc84b7776173d105` moves credential reads off async backend workers, bounds the wait at 30 seconds and lets Stop cancel run setup before integrations materialize. A process sample confirmed synchronous Keychain reads blocked backend workers while the Mac was locked. The previous five-minute trial never reached a model request and remains unscored.

Locked-Mac checks passed: get_thread 9 ms, ping 963 ms, credential status returned false after 30.965 seconds, and Stop released pending run setup in 38 ms with zero tools. After unlocking, credential status returned true; the native app showed Go connected, all eight coding attachments and the stopped chat. This verifies responsiveness, not cancellation of the native Keychain operation itself. Fresh workspace coverage ran 371 passing tests, two opt-in ignored tests and one excluded synthetic Keychain test; Rust coverage 20,670/23,232 = 89.0%. Clippy, rustfmt, build, Ruff and 14 dashboard tests passed. Evidence: `/tmp/themis-keychain-read-20261010/` and `/tmp/themis-keychain-read-final-*.log`.

The sequential 200k coding retest used the same eight attachments (1,623,145 bytes), model and baseline. The prepared summary estimated 166,637 input tokens with an 8,192-token output cap. Both summary attempts returned HTTP 429; the harness retried after one second and wrote fallback state after 29.410 seconds. The observer stopped at 42.37 seconds, with one compaction and zero tools/edits. All eight original attachments, one captured snapshot, requirements and tests passed preservation checks. This case is unscored; no next case was dispatched. A small control request to the same model returned READY in 20.16 seconds, confirming access worked but not establishing why the large request was rejected.

The adapter was found to discard Retry-After headers. A regression reproduced a 45-second header becoming None. The patch preserves seconds and HTTP-date hints for typed HTTP errors; summarization already observes these hints under the same five-minute deadline. Ordinary answer retry remains fixed at one second. The observed 429 metadata did not retain response headers, so this bug is not proof that a header was sent or that fixing it resolves the large request rejection. No output caps were changed.


Retry-header verification: `cargo test -p themis-core --lib` passed all 143 tests. The focused CLI case `cargo test -p themis-desktop --test attachments_e2e cli_fitting_compaction_uses_one_update_and_keeps_originals` passed, including a 429 then successful retry and preservation across restart. The focused deadline/Stop regression passed with Retry-After 45. Ruff and all 14 dashboard tests passed. Dashboard refresh retained overall cyclomatic 2.56, cognitive 2.24 and maintainability 73.4; providers.rs changed from 2.18/1.17/67.9 to 2.19/1.20/67.8, including inline tests. The available 89.0% Rust coverage predates this retry-header patch; no fresh coverage claim is made for it. Read-only review found no concrete findings.


## Rebuilt retry-header trial: 200k coding passed

Commit `516114001a08b65775b377a79755e93e4876cdd0` passed Clippy, rustfmt and the development custom-protocol rebuild. Ad-hoc codesign verification passed. Rebuilt binary SHA-256: `ef2e968f8ae8d7e1b2a33f04f4af5a1decc302759af8206537023c86b470abd2`. Evidence: `/tmp/themis-retry-after-20261010/build.json` and `/tmp/themis-retry-after-{core,deadline,cli,clippy,build}.log`.

The sequential 200k coding case finished naturally in 1,447.34 seconds: one successful compaction, no fallback, 39 tools, all six target modules edited. The summary completed in 72.137 seconds with an estimated 166,636-token prepared input. No summary retry was needed; this success cannot be attributed causally to honoring Retry-After. Normal model generation and correction of generated syntax/rounding mistakes dominated elapsed time.

Independent `python3 -m unittest discover -s tests -v` passed all 113 tests (16 unchanged supplied tests plus 97 model-added cases). The predeclared holdout completed all 210 checks, including 200 randomized ledgers. All four reference blocks were byte-identical; requirements and supplied tests were unchanged; imports remained stdlib-only. All eight full original attachments and 40 previously captured snapshots passed preservation checks. The copied holdout helper's stale project path was corrected before scoring; prior single-pass and shell-history helpers already used their proper trial paths, and rerunning them confirmed their previously reported failures.

The native app visibly showed the completed result, all attachments, 39 activities, and an idle composer. Network byte counters showed incoming data during two long ordinary-generation waits; they do not establish the content or prove a specific reasoning issue. Ordinary OpenAI-shaped requests currently omit an explicit output cap; no new cap was introduced in this patch. Evidence: `/tmp/themis-retry-after-20261010/coding/200k-{history,metrics,verification,snapshot-hashes}.json`, tests/oracle logs, and preservation helper output. The 24k coding and both narrative cases remain pending at this point.


## Sequential 24k coding stopped at fallback

The same build/model/baseline at 24k prepared an estimated 8,904-token input and capped the first summary at 1,654 output tokens. It was truncated; the sole retry requested 3,308 tokens. Summarization failed after 118.533 seconds without exhausting its five-minute deadline. A fallback checkpoint was saved at 118.598 seconds; the observer stopped at 125.84 seconds, with zero tools or edits. All eight full originals, one captured snapshot, requirements and supplied tests passed preservation checks. No narrative case was dispatched. The existing diagnostics did not identify the retry's final error category or token usage; the next patch adds numeric/categorical diagnostics before changing budget behavior. Evidence: `/tmp/themis-retry-after-20261010/coding/24k-{history,metrics,stopped-issue}.json` and `diagnostics.jsonl`.


## Summary failure diagnostics

The 24k reproduction needs evidence before a budget change. Nonstream Chat Completions truncation now records the configured output allowance, reported completion/reasoning counts and public text character count. Summary failures record only fixed categories and HTTP status, including an empty retry response. Responses/Messages reject truncation earlier and do not yet emit this usage event. No prompt, reasoning or response text is logged. A regression verifies both counts survive redaction and private sentinels do not appear.

Fresh core verification: 144 tests passed; Ruff and all 14 dashboard tests passed. Dashboard averages remain 2.56 cyclomatic, 2.24 cognitive, 73.4 maintainability across 107 files. Providers: 2.17/1.18/67.7, runtime: 2.67/2.48/68.1; inline tests remain included. Available 89.0% Rust coverage predates this diagnostics patch; frontend coverage remains unavailable. Read-only review found no remaining findings.

Diagnostic build `9074ee8` passed Clippy, the shared-server CLI retry/preservation regression, rustfmt and custom-protocol rebuild; ad-hoc codesign verification passed. Binary SHA-256 `e40623e2a598372e4af3339b97a3dea1a61874ead8b114f798387a8b0469d819`. The 24k reproduction reported completion/reasoning counts 1654/1653 and 3308/3307, with zero public summary characters in both attempts. It fell back after 146.083 seconds (deadline not exhausted); observer stopped at 156.91 seconds with eight tools and no edits. Collector lag allowed those tools before the fallback was observed. All eight full originals and captured snapshots passed preservation; temporary configuration was removed after idle. This confirms reasoning consumed the summary allowance in this trial. Evidence: `/tmp/themis-summary-usage-20261010/`.


## Internal summary thinking fix

Commit `9518769` adds task-local summary purpose around the sole production summary call, including its retry, and sends LongCat 2.5's documented `thinking: {"type":"disabled"}` only for internal summaries. Ordinary answers and other models keep existing options. No output cap was raised and no shared ordinary-output ceiling has been implemented. Successful nonstream Chat Completions summaries also retain numeric usage metadata. Provider tests verify scoped requests, ordinary defaults, another model's unchanged options and metadata; the shared-server CLI regression verifies the summary flag and its absence on ordinary requests through compaction/restart.

Fresh verification: 145 core tests, the focused CLI case, Clippy, rustfmt, custom-protocol build, Ruff and 14 dashboard tests passed. Dashboard overall averages remain 2.56/2.24/73.4; providers 2.20/1.26/67.9, runtime 2.67/2.48/68.1. Inline tests are included. Available Rust coverage still predates this patch (89.0%); frontend coverage is unavailable. Read-only review found no concrete findings. Binary SHA-256 `213d77174372dc9e07e83dcfb202a0bb0c6ff6b9dc5767bcf30324bee4318690`; codesign verification passed.

The fresh 24k coding trial's first summary completed on its first attempt in 15.875 seconds, using 505 completion tokens and producing 2,131 text characters within the unchanged 1,654 cap. The provider omitted the reasoning-token count on success; this is not a measured zero. This observed improvement is consistent with the supported thinking flag being honored. The coding trial remains running; no acceptance pass is claimed yet. Evidence: `/tmp/themis-summary-thinking-20261010/`.

The 24k coding trial on `9518769` ended after 629.38 seconds with an ordinary stream timeout, five successful summaries (no retries/fallback) and 29 tools. The baseline remains byte-identical: no target source edits were made. The final public statement said it would implement the fixes, but no complete write call arrived before the timeout. Independent supplied tests retain 20 failures/four errors; the holdout aborts on the first randomized case (`KeyError: jpy`). All eight full originals, 34 captured snapshots, supplied tests/requirements and reference tables passed preservation. Temporary configuration was removed after idle. No next live matrix case was dispatched. The shared no-reasoning policy requested afterward is under verification; this trial predates that policy but its selected LongCat summary path is the same.


## Shared internal no-reasoning policy

The user requested reasoning disabled for internal summaries regardless of the selected answer model, and approved LongCat with thinking disabled for models without a verified off control. The shared provider path retains LongCat 2.5 or the exact GPT 5.6/6 Luna IDs (the current Go catalog advertises effort `none`); all other/unknown models conservatively route summary requests to `longcat-2.5-preview-free`. The request-local clone clears selected effort, preserves client/credentials/session, and selects the effective protocol for both requests and response parsing. Ordinary answers retain their model/effort. This does not assert that every fallback model requires reasoning; additional off controls can be added after verification. GPT off controls are wire-tested, not live-verified in this audit.

The red regression caught selected high reasoning leaking into LongCat's summary request. The final matrix covers ten model IDs and all three normal protocols, verifies fallback, output caps, retained answer options, correlation headers and effective-model diagnostics. The shared-server CLI case uses Mimo answers and LongCat summaries, including a 429 retry, original preservation and restart.

Fresh full verification: `cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten --test-threads=1` passed **374 tests**, two opt-in ignored tests and one excluded synthetic Keychain test. Rust coverage: **21,023/23,586 = 89.1%**. Core 145, focused CLI, Clippy, rustfmt, custom-protocol build, Ruff and 14 dashboard tests also passed. Dashboard overall remains 2.56/2.24/73.4 across 107 files. Providers changed from 2.20/1.26/67.9 to 2.23/1.34/67.9, including the expanded inline regression matrix; runtime remains 2.67/2.48/68.1. Frontend coverage is unavailable. Read-only review found no concrete findings. Logs: `/tmp/themis-summary-policy-{core,cli,clippy,coverage,build,dashboard-tests}.log`.

After the ordinary LongCat stream timeout, the user approved Step 5 Preview Free for coding, with LongCat no-thinking summaries. The fresh sequential protocol uses Low ordinary effort and byte-identical fixtures. This is a changed model/effort, not a controlled before/after causal comparison. Evidence directory: `/tmp/themis-summary-policy-20261010/`; no fresh live result is claimed before dispatch.


## Latest shared-policy build and sequential trial

Build commit `f6fc3abf09c8e23ad228aff99f8c8c01343f4f32`; binary SHA-256 `8e3da93ccaefa3406bc2f3c201b131ca3a55fc6b20856698e4f4144daa008e7e`. Ad-hoc codesign verification passed. The normal app was refreshed while the shared-server model run continued; native inspection showed the active `Summary policy 24k coding` chat, all eight attachments and the selected Step 5 model. The CLI sends Low effort per run; the native composer displays its separate Default selection, so the UI is not evidence of effective Low effort. Frontend activity elapsed time restarted on refresh; that display behavior is outside this compaction patch.

The fresh 24k trial's first two summaries completed in 28.054 and 32.569 seconds on LongCat, with 1,127 and 1,177 completion tokens within the unchanged 1,654 cap. Both succeeded without retry or fallback. Reported reasoning counts are null, not measured zero. The model subsequently patched four large target modules through Python. The run remains in progress; no completed acceptance result is claimed. The 200k coding and both narrative fixtures are prepared but no second model run has been dispatched.


### 24k artifacts passed; continuation audit stopped

The latest trial was stopped at 663.70 seconds after five successful checkpoints, one permitted truncation retry, no fallback and 35 tool calls. All six target modules were edited. Independent supplied tests passed (16/16), and the predeclared holdout passed all 210 checks, including 200 randomized ledgers. All four reference tuples were byte-identical; requirements and supplied tests were unchanged; imports remained stdlib-only. Eight complete originals and 40 previously captured snapshots passed preservation. This is an artifact pass, not a naturally completed autonomous run: after passing the tests the model invented `.thisis-*` file paths and restarted discovery, so the observer stopped it for investigation. It never completed the requested additional edge checks or a final answer. Evidence: `/tmp/themis-summary-policy-20261010/coding/24k-{metrics,verification,preservation,manual-stopped-issue}.json` and test/oracle logs. The temporary 24k override was backed up and removed after idle.

Checkpoint inspection found a concrete stale-state gap: checkpoint three still said no changes were completed after four source edits; checkpoint four said tests had not run although the retained tool result showed 16 tests passed. The summary updater received only messages being evicted, excluding the newest retained observations. Later checkpoint five correctly retained completed edits and passing tests, so omitted observations do not by themselves explain every erroneous path or reread. The patch includes recent observations in the same locally bounded packet; retained verbatim tool pairs, output caps, one-summary/one-retry limits and originals are unchanged. The focused wire regression failed on the missing newest test result before the patch and passed afterward at 24k and 200k. Live behavior on the patch is still unverified.


Fresh tail-observation patch verification: all 145 core tests passed, plus `cargo test -p themis-desktop --test attachments_e2e cli_fitting_compaction_uses_one_update_and_keeps_originals`, including the newest retained record, summary retry, provider separation, original preservation and persisted restart. Ruff and all 14 dashboard tests passed. Dashboard refresh still analyzes 107 files with overall averages 2.56 cyclomatic / 2.24 cognitive / 73.4 maintainability. Runtime: 4,375 LOC, 2.68 / 2.48 / 68.1, including expanded inline tests (prior 4,342 LOC, 2.67 / 2.48 / 68.1). Available 89.1% Rust line coverage predates this focused patch; frontend coverage remains unavailable. Read-only review found no production regression. The existing no-archive guard now also covers sampling of recent retained fields, so legacy callers without an evidence directory can fail earlier; their original conversation is preserved, and production evidence-backed calls are unaffected. Logs: `/tmp/themis-summary-tail-{core,cli,dashboard-tests}.log`; metrics: `/tmp/themis-summary-tail-20261010/metrics.json`.


## Cold-start effort validation

Tail-observation build `1a1644a` rebuilt and passed ad-hoc codesign verification (binary SHA-256 `e8ec8cdfe258a7cbf44698310f6e3b87fcdd88c9edf7ce251d583e0754a95f3d`). The next 24k Step 5/Low trial was rejected before tools: the backend starts with an empty in-memory Go catalog and treated that as unsupported effort. Refreshing the actual catalog confirmed Step 5 advertises low/medium/high. A brief Default-effort control was stopped after 26.11 seconds, one compaction and zero tools; neither setup case is scored as a coding trial. No source edits occurred. Evidence: `/tmp/themis-summary-tail-20261010/coding/24k-invalid-effort-*`, `24k-launch-*`, `24k-setup-default-*`, and `models.json`.

Commit `082caf6` reuses the existing catalog loader before effort validation in the setter and run setup. The existing Stop selection also covers that fetch, and no thread/catalog lock is held across network waits. Regression tests failed before the patch and passed afterward: load failures remain explicit loading errors rather than unsupported-effort claims, and Stop cancels a delayed fetch within two seconds after the request starts. The CLI exercises both entry paths through an isolated shared server using synthetic credentials.

Fresh verification: `cargo test -p themis-desktop --lib -- --skip entered_keys_survive_new_store_and_can_be_forgotten` passed 100 tests (one synthetic Keychain exclusion); `cargo test -p themis-desktop --test attachments_e2e` passed all nine; workspace Clippy, rustfmt, diff checks, Ruff and all 14 dashboard tests passed. Read-only review found no concrete regression. The dashboard still analyzes 107 files at 2.56 cyclomatic / 2.24 cognitive / 73.4 maintainability. State: 2,284 LOC, 1.59 / 0.48 / 67.9; run: 452 LOC, 3.38 / 3.82 / 78.8; threads: 549 LOC, 2.16 / 0.89 / 78.9, including inline test code. Available 89.1% Rust line coverage predates the tail and cold-catalog patches; frontend coverage remains unavailable. Logs: `/tmp/themis-cold-catalog-{desktop,attachments,clippy,green,cli,dashboard-tests}.log`; metrics: `/tmp/themis-cold-catalog-20261010/metrics.json`.

The new cold-catalog build and live retest are pending at this point. Both narrative budgets and the latest-build 200k coding case remain unverified.


The cold-catalog rebuild passed in 18.57 seconds; ad-hoc codesign verification passed. Build `082caf693f974bdfe0cceb3d6d869bf3462c4f2c`, binary SHA-256 `8503f4f0f1e568665d666687bff8f4e172a72e0a01b0bda07188fe5eaa0e007b`; metadata `/tmp/themis-cold-catalog-20261010/build.json`. Fresh 24k coding chat `330ebea0-ef01-4eaf-9216-142c0caa0f08` was submitted alone with Step 5/Low and eight unchanged attachments. The Mac locked again before native inspection; live acceptance is pending.


The submitted cold-start trial stopped before a model request with `Credential lookup timed out; unlock the OS credential store and retry`. Its chat returned idle; no tools or source edits occurred. This is an unscored locked-Keychain setup attempt, not a compaction failure. The existing credential deadline prevented an indefinite backend freeze. Evidence is preserved under `coding/24k-locked-*`; no follow-on live case was dispatched. Native verification and the next live run require unlocking the Mac.
