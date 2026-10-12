# Shared compaction and recovery verification — 2026-10-10

Implementation: `8fa909c` on `codex/bounded-tool-context`. The two changes are implemented and the rebuilt app was exercised with a live model. Automated checks pass, but live acceptance is not fully met.

## Changes

Persistently truncated summarization sections are halved on UTF-8 boundaries, at most three levels, while retaining successful sections and the existing concurrency/deadline/Stop boundaries. Fitting updates that truncate use the same section path. Source byte coordinates use exactly the archive serialization.

Shared JSONL archives distinguish original observations, Assistant navigation, generated recovery output and mixed Tool batches. Entry-level provenance preserves original siblings. Automatic searches and the supplied Python recipe avoid marked recovery copies. Optional `decode_tool_payload` decodes Tool envelopes once; default raw reads remain compatible. Retained locations preserve the decode flag. Python decoded Tool matches use `find` rather than potentially incompatible numeric offsets. Legacy snapshots and scripts lacking the archive-path literal remain classification limits.

## Automated checks

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test -p themis-core --lib`: **143 passed**.
- `cargo test -p themis-desktop --test attachments_e2e`: **7 passed**. Includes the actual constructed system message and Python recipe delivered through the shared CLI/provider request path.
- `cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten --test-threads=1`: **367 passed**, two opt-in tests ignored, one host Keychain test explicitly excluded. Fresh Rust lines: **20,995/23,539 (89.2%)**. Frontend line coverage is unavailable; this does not establish Keychain persistence.
- `npm --prefix web run lint`, `npm --prefix web test`, `npm --prefix web run build`: passed; **306 tests / 29 files**. Existing Vitest deprecation/localStorage and Vite chunk-size warnings remain.
- `python3 -m unittest quality_dashboard.test_dashboard`: **14 passed**, separate from product tests; `ruff check quality_dashboard`: passed.

An earlier ordinary serial workspace run stalled in `agent_can_disable_broken_enabled_mcp_without_losing_chat` for over three minutes and was interrupted. The fresh coverage run passed that same test and the complete workspace. The stall's cause is unconfirmed; the interrupted attempt is not a pass.

Regression failures were observed before fixes for adaptive subdivision, recovery provenance, fitting-update provenance, untyped JSONL compatibility, decoded-coordinate retention, recovery-cache exclusion, mixed batches and Python/Rust offset compatibility. A read-only review's four concrete findings were addressed and re-reviewed without remaining actionable findings. Review did not replace test execution.

## Build and protocol

`cargo build -p themis-desktop --bins --features tauri/custom-protocol` passed after the frontend build. The development bundle was updated atomically, ad-hoc signed, and verified with `codesign --verify --deep --strict`. The old normal backend was stopped only after checking all 65 known chats were idle; unrelated QA servers were preserved. Native frontend and its newly spawned normal backend launched successfully.

Bundle: `/Users/cutedandelion/Documents/ChatGPT/ThemisOS/target/debug/bundle/macos/ThemisCode.app`. Binary SHA-256: `e98ab57f7d46d22a020a601f70a00177524482087200b332a6b46eb15957d91f`. This is a development custom-protocol build, not a release package.

Evidence root: `/tmp/themis-shared-recovery-20261010/`. Protocol was frozen in `protocol.json` before live requests. Same provider/model as the prior audit: OpenCode Go `longcat-2.5-preview-free`, default reasoning effort. Project-only budgets are 24,000 and 200,000 estimated tokens, two recent messages, 100 tool turns. No global runtime, MCP or skill registry changes. At most two live chats run concurrently; elapsed times are observational, not controlled latency comparisons. No corrective operator prompts or success-selecting retries. Observers may stop coding at 25 minutes, each initial narrative part at 12 minutes, and the separate recovery phase at 20 minutes.

Coding: eight attachments, **1,623,145 bytes**, six broken modules, four large immutable reference tables. Both fresh baselines fail the 16 supplied tests (20 subtest failures and four errors). Acceptance requires natural completion, those tests, 210 independent checks, unchanged requirements/tests/staged originals and byte-identical reference tables.

Narrative: six consecutive renamed Clarissa parts, **5,042,808 bytes**; each part alone forces compaction at both budgets. Initial phase forbids tools/rereads; five exact acknowledgements, then chronological retelling, 24 fact questions and four exact short quotations. Separate recovery permits bounded local rereads and requires sources. Rubric: 24 facts scored 1/0.5/0, ten chronology pairs, four quotations normalized only for whitespace. Hash audits check originals/staged files and previously captured snapshots. Recognizable source material can benefit from prior model knowledge; this is a synthetic audit, not a general quality estimate or runtime gate.

## Live outcomes

### Large multi-file coding

| Budget | Natural completion | Time | Tools / compactions | Independent artifact checks |
| --- | --- | --- | --- | --- |
| 24k | No; observer stopped it at 25 minutes | 1,503.24 s | 49 / 8 | 16 supplied tests and 210 holdout checks pass |
| 200k | Yes | 614.76 s | 45 / 1 | 16 supplied tests and 210 holdout checks pass |

Both repaired all six modules using the standard library, preserved requirements/tests and all four reference blocks byte-for-byte, and retained all eight staged/full JSONL attachment originals. Previously captured snapshot hashes were unchanged: 60 at 24k, 47 at 200k. The independent holdout includes 200 randomized ledgers and precision/type/CLI cases; the model's claimed extra tests are separate evidence.

Coding verification: `python3 -m unittest discover -s tests -v` from each coding project; `python3 /tmp/themis-shared-recovery-20261010/coding/24k-oracle.py` and `200k-oracle.py`; `python3 /tmp/themis-shared-recovery-20261010/coding/verify_preservation.py 24k` and `200k`. Holdout and preservation checks passed again after all live chats were idle.

At 24k, a 7,992-byte repair script survived compaction and ran successfully. After the supplied tests passed, the model repeated hash verification and inspected already repaired source. The raw Tool journals show **seven consecutive identical shell calls and identical outputs**: the commands executed each time, rather than being UI replay or cached execution. The existing repeat reminder is added after execution. The model's claim that the shell was caching results is unsupported. It eventually moved beyond the exact repeated call but did not finish before the observer deadline. Correct files therefore do not count as an autonomous end-to-end pass.

The 200k run timed out its initial compaction at approximately 240 seconds and saved 15 completed section notes, then completed the repair. At 24k, diagnostics record five successful compactions and two fallbacks; its eighth compaction was interrupted by the observer. Repetition also occurred after successful compaction, so fallback alone does not explain it.

### Narrative continuity — initial phase

| Budget | Final turn | Tools / compactions | Facts | Chronology | Exact quotes |
| --- | --- | --- | --- | --- | --- |
| 24k | Provider stream ended before completion; no final answer | 0 / 6 | Unscored | Unscored | Unscored |
| 200k | Natural final answer, but all-source abstention and no retelling | 0 / 6 | 0/24 | 0/10 | 0/4 |

All twelve initial narrative compactions produced fallback checkpoints despite adaptive subdivisions. The 24k first turn reported unavailable prose instead of acknowledging it; the next four acknowledged receipt. At 200k, the first five acknowledgements complied exactly, but acknowledgement is not evidence of retained narrative. Its final response explicitly lacked the story and supplied no facts or quotations. The 24k final stream failed after 257.13 seconds; an earlier acknowledgement is never graded as its final answer. No story hallucinations were rewarded.

All six full originals remain in JSONL and all source/staged hashes match at each budget. Previously captured snapshot hashes are unchanged: nine at 24k, twelve at 200k. This demonstrates durable preservation, not working-context retention.

### Narrative continuity — separate recovery phase

The 200k recovery searched local originals directly, including root story files, rather than recursively mining escaped recovery copies. It made **24 tool calls, no additional compaction**, then failed after **212.82 seconds** with provider HTTP 500 and malformed function-call JSON (invalid escape). No final retelling was produced; semantic accuracy is **unscored**. The malformed call failed before execution. This is not evidence of an oversized tool-output failure, and the provider/model layer responsible was not isolated.

The 24k recovery reached its **1,200.68-second observer limit** after **45 tool calls and ten additional compactions**, without a final answer. Nine recovery compactions completed successfully; the tenth was interrupted by the observer. Semantic scores are **unscored**, not zero. It extracted/searched originals and read staged attachments directly, but continued retrieving instead of finishing. One shell call failed because a whole command line was supplied as the executable name. Successful compaction therefore did not guarantee autonomous completion here either.

The observer's immediate history snapshot raced with terminal-event persistence and ended at `context_compacting`; a fresh shared-server history read after all chats were idle confirms `failed` / “Stopped by you.” Both versions are retained. The refreshed history alone is used for final-phase verification. All six full originals, source/staged hashes and nine previously captured snapshot hashes still pass.

| Recovery budget | Outcome | Time | Tool calls | Additional compactions | Semantic scores |
| --- | --- | --- | --- | --- | --- |
| 24k | Observer stop; no final retelling | 1,200.68 s | 45 | 10 (nine completed) | Unscored |
| 200k | Provider malformed tool-call JSON; no final retelling | 212.82 s | 24 | 0 | Unscored |

### Interpretation and comparison

Adaptive subdivision executes in live diagnostics; it is not enough to guarantee a usable final summary within the shared deadline. Original-first retrieval is observable, but bounded output and preserved originals do not guarantee task completion or semantic continuity. This trial leaves ordinary repeated tool execution and narrative summarization reliability unresolved.

The previous coding audit completed naturally at both budgets (24k: 755.04 s, 34 tools, five compactions; 200k: 362.82 s, 39 tools, one compaction). This trial is worse on autonomous completion at 24k and slower at 200k. A single uncontrolled free-provider run cannot isolate whether the implementation caused the difference. The previous narrative audit also failed initial continuity at both budgets; its recovery phases produced no final retelling. Do not claim measured efficiency or retention improvement from these runs.

Evidence: `coding/*-metrics.json`, `*-oracle.log`, `*-preservation.json`, `24k-repeat-audit.json`; `narrative/*-metrics.json`, `*-verification.json`, `*-scores.json`, and original histories under the evidence root. Histories and artifacts are distinct from the bounded metadata-only diagnostic log.

| Product test chat | ID |
| --- | --- |
| 24k coding | `f34ffa3d-b5c0-4fb3-bdc4-a1a3f2786e0f` |
| 200k coding | `090c9132-ef32-43ac-8831-7703366b0ebc` |
| 24k narrative | `d74ddd0f-8fdd-4989-aa06-bab023c4d6b6` |
| 200k narrative | `37f478e6-8d0b-4d46-a4ec-cfcc29e561cc` |

### Native checks and cleanup

The rebuilt native app displayed the coding attachments/activity, the completed 200k coding answer and the stopped 24k coding run. The Mac locked before the final narrative UI inspection; that remaining native view is unverified. Attachments were submitted through the real shared-server CLI path; this audit did not repeat the native attachment-picker interaction.

All four chats were confirmed idle through `themis call get_thread -`. The four exact temporary `.themis/config.jsonc` test overrides were validated, backed up under `removed-configs/` in the evidence root and removed. No global runtime, MCP or skill registry was modified. Test projects, conversations, repaired files and original evidence remain. The diagnostic collector and all four test pipelines exited; the rebuilt normal app backend and quality dashboard remain running.

Final narrative hash checks: `python3 /tmp/themis-shared-recovery-20261010/narrative/verify.py 24k recovery` and the equivalent `200k recovery` passed. Scoring files explicitly use null for technical failures or observer stops. Dashboard self-tests (14), Ruff and a metrics refresh passed again after live testing. `git diff --check` passed before the report commit.

## Code health

Dashboard refreshed through `http://127.0.0.1:4178/api/metrics`. rust-code-analysis 0.0.25 analyzes 107 product files; mean cyclomatic/cognitive/maintainability **2.56 / 2.24 / 73.4**. Runtime **4,808 LOC, 2.58 / 2.30 / 68.3**, compared with **4,433 LOC, 2.56 / 2.28 / 68.3** before this change. Tools **2,529 LOC, 2.06 / 0.83 / 69.8**. Inline Rust tests count in LOC/AST. The runtime's size and slight complexity increase remain a maintenance concentration; no claim of metric improvement. Coverage increased from 89.0% to 89.2%, but coverage is not semantic retention evidence.
