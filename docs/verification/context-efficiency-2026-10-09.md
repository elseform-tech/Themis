# Context efficiency verification — 2026-10-09

This work continues `codex/bounded-tool-context`. It is a synthetic live-model audit, separate from scripted-provider regression checks. The [preceding audit](bounded-context-live-2026-10-09.md) records earlier findings.

## Changes

- `4ad1559`: bounded historical shell/edit/test observations survive lossy summaries and restart; exact unchanged read results receive bounded reminders; summary requests adapt to the available input/output allowance; unified-diff guidance and rejection errors are explicit.
- `a0224d3`: recurrent fallback rebuilds generated recovery envelopes and caches once rather than recursively nesting prior checkpoints.
- `9d03548`: checkpoint caches share the actual serialized remaining budget. Completed invocation arguments above 2,048 bytes become historical receipts with a 512-character preview and exact original-record coordinates. Mandatory instructions/state are preserved; impossible mandatory context still fails explicitly with originals retained.
- `5b90d98`: fallback guidance continues retained work and retrieves only missing facts for the next unresolved action, removing the unconditional request to recover goals/constraints before substantive continuation. `e7c668f` aligns the existing permission assertion and guide.

The joint-budget and recurrent-fallback regressions failed before their fixes. The latter exercises eight recurrent failures. The fallback-guidance regression checks the actual model continuation request through the CLI/shared server; it failed on the former wording before passing. Review found no actionable issues in these shared fixes. Exact result comparison and guidance do not enforce semantic progress or detect paraphrased repetition.

## Rebuild and native checks

Ran `npm --prefix web run build` and `cargo build -p themis-desktop --bins --features tauri/custom-protocol`. Vite retains its existing large-chunk warning. The native executable was copied atomically into `target/debug/bundle/macos/ThemisCode.app`, signed ad hoc, and verified with `codesign --verify --deep --strict`. This is a development build, not a signed release.

Final executable SHA-256: `602c7e3e5907e596cfcba9672035dd3a1756f806546ef999b0834207ebe83a86`. All 59 persisted chats were idle before the last restart. The final bundled backend started after the executable was replaced, and the native UI loaded `tauri://localhost`. The 24k fact test and final large coding result were observed completed in the native UI; the fresh difficult 24k coding diagnostic was observed stopped. Unrelated QA backends were preserved. The final test-only source assertion does not change the production executable.

Live tests use the production shared server through `themis`, existing authenticated OpenCode Go access, and synthetic projects. Credential values were not read or printed. YOLO and context overrides are scoped to test chats/projects; user defaults are preserved. A 25-minute observer limit distinguishes operator Stop from natural completion/failure.

## Live acceptance and failures

The coding fixture contains eight Python modules, four large immutable reference tables, and interacting validation, canonicalization, revision conflict, status-ordering, per-row currency rounding, sorting and CLI requirements. Baseline supplied tests fail. The independently prepared oracle has 210 checks: 200 randomized ledgers plus precision/global Decimal-context, validation and CLI checks. Supplied test hashes and all four reference blocks must remain unchanged. Synthetic padding establishes size pressure, not real-world application complexity.

| Case / build | Outcome | Time / tools / compaction starts |
| --- | --- | --- |
| 1,623,145-byte coding, `4ad1559`, Longcat | Natural completion; 16 supplied tests + 210 independent checks pass; six source modules repaired; all reference blocks byte-identical | 370.55s / 35 / 1 |
| Same large baseline, `a0224d3`, Longcat | Natural completion; 16 + 210 checks pass; reference blocks and supplied tests unchanged | 335.03s / 38 / 1 |
| 543,325-byte coding at 24k, before nesting fix | Natural checkpoint-budget failure; independent supplied tests also fail | 424.49s / 17 / 3 |
| Fresh 24k coding, `a0224d3` | Natural checkpoint-budget failure after a large completed write; supplied 16 tests and 210 oracle checks pass, but the model's additional suite has 5 failures / 2 errors; no overall acceptance pass | 535.08s / 19 / 4 |
| Fresh 24k coding, `9d03548`, Longcat | Provider stream timeout before edits | 317.49s / 12 / 1 |
| Same 24k chat resumed, `9d03548` | User intentionally stopped repeated inspection statements; no edits; budget failure did not recur | 414.08s more / 17 more (29 total) / 3 total |
| Fresh large coding, `9d03548`, Longcat | Provider stream timeout before edits | 405.23s / 11 / 1 |
| Same pristine large source, new Step-5 chat, low effort, `9d03548` | Provider stream timeout before edits | 427.88s / 11 / 1 |
| **Final-build 24k facts/write**, Step-5, low effort | **Natural completion; all six independent fields exact; input and staged copy unchanged** | **114.06s / 12 / 1** |
| **Final-build large coding retry**, Longcat | **Natural completion; supplied 16 + independent 210 + model edge checks pass; six modules repaired; original tables/tests unchanged** | **269.82s / 38 / 1** |
| **Final-build fresh 24k coding retest**, Longcat | **Operator stopped repeated inspection after script preparation; no settlement edits applied; acceptance fails** | **574.31s / 18 / 4** |

The first two large coding successes used fallback recovery, not successful working-summary retention: fallback took 85.00s and 100.30s, respectively. Complete originals remained available. The first successful run had one rejected malformed patch and several nonzero shell/Git commands; transport success is not command/test success. No controlled performance claim follows from these variable model runs, which also overlap in parts with other verification.

The final large retry also used fallback recovery (46.62s, one partial section note). All eight complete attachments occur in its initial snapshot, all seven staged Python baselines match their pre-edit hashes, its previously recorded initial-snapshot hash is unchanged, and the requirement file is unchanged. AST import inspection finds only stdlib imports. The model's independent edge-check script also passes. Acceptance is scoped to the listed oracle; amount-string lexical variants such as exponent notation are not covered by its 210 cases.

The stopped 24k coding retry had three fallback checkpoints with one copy each of the generated read/action blocks, so recursive nesting was fixed. It nevertheless repeated inspection narratives without editing; exact unchanged-result reminders did not prevent that semantic loop. The user confirmed the Stop was intentional. That chat remains stopped. The subsequent wording change is a plausible contributor fix, not proof that the difficult 24k coding loop is eliminated. The fresh final-build 24k coding retest confirms that this loop remains reproducible.

The fresh difficult 24k coding retest wrote `_repair_tmp.py` (5,614 bytes) but did not execute it. After the third saved checkpoint, it said “Before running the patch” and reread requirements/tests, then started a fourth compaction. The first two saved checkpoints were fallback recovery; the third contained a working summary whose next action still asked for rewriting the modules, while the completed write receipt retained the prepared script. This is an observed continuation failure despite available action evidence, not proof of one specific model-internal cause. The operator stopped it at 574.31s; the UI displayed “Stopped by you.” All eight source files, supplied tests and eight previously hashed snapshots remained unchanged. Independent supplied tests still fail (16 run, 20 subtest failures / 4 errors), and the independent oracle exits on the baseline currency bug. No checkpoint-budget error occurred. A broad recursive home-directory search succeeded but wasted time and exposed other synthetic fixtures; this was not an isolated model benchmark. No evidence of copying those solutions was found before Stop.

The final 24k facts/write case attaches **812,333 bytes of JSONL**, forcing compaction under an explicit 24,000-token project budget. It asks the model to compute and write six exact fields from the original data: record count, active-unit sum, distinct groups, highest control revision/Unicode label, and full-byte SHA-256. The precomputed oracle matches every field. The model saved `result.json`, checked it, and finished naturally. Summary output was truncated, so a fallback checkpoint was saved after **18.49s** with partial navigation retained. The complete attachment occurs in the initial JSONL snapshot, and seven previously recorded snapshot hashes remain unchanged. This validates one low-budget attachment/recovery/write journey; it is not a pass of the stopped 24k multi-file coding case or a recurrent semantic-progress guarantee.

An earlier exploratory large run was intentionally stopped before acceptance, with no edits. It is not counted as a pass or natural model failure. Initial final-build fact-test setup also rejected the requested effort before a run began; refreshing the provider model catalog allowed the same chat to start. This setup retry is separate from model execution time.

## Automated checks and code health

- `cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten`: 360 passed, two opt-in tests plus one documentation example ignored, one host Keychain test explicitly excluded. This ordinary workspace run precedes the fallback wording-only follow-up.
- After the wording/assertion follow-up, `cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten`: **360 passed**, two opt-in tests ignored, one host Keychain test excluded. Fresh Rust line coverage: **20,482/23,014 (89.0%)**. An intervening run found the old permission-phrase assertion; it was corrected without removing the permission/cancellation/source-preservation checks.
- Final `cargo test -p themis-desktop --test attachments_e2e`: all six pass, including recurrent compaction/restart, transient retries, MCP large-result bounds, original preservation and the constructed recovery request path. Final core coverage includes 137 unit tests and 17 runtime integration tests (one opt-in integration test ignored).
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `git diff --check` pass.
- `npm --prefix web test`: 306 tests across 29 files pass; web lint/build pass. No frontend source changed during this follow-up.
- `python3 -m unittest quality_dashboard.test_dashboard`: 14 pass. `ruff check quality_dashboard`: pass.

The refreshed dashboard analyzes 107 product files using rust-code-analysis 0.0.25. Mean cyclomatic/cognitive/maintainability: **2.56 / 2.24 / 73.4**. Runtime: **4,388 LOC, 2.57 / 2.27 / 68.4**; tools: 2,385 LOC, 2.01 / 0.79 / 69.8; patch: 332 LOC, 3.32 / 2.34 / 76.7. Inline Rust tests count in LOC/AST. Runtime complexity/size grew and its maintainability measure decreased from 68.6 before the joint assembly fix; this is a maintenance concentration, not a claimed score improvement. Frontend line coverage remains unavailable.

## Evidence and cleanup

Final 24k facts chat: `5161a9b4-107e-46db-8863-433265f978cc`, project `/Users/cutedandelion/ThemisOS/Projects/Context Efficiency 24k facts final 20261009`.

Earlier successful large chats: `79775a68-6fc9-4ace-9ed9-cdec157d8b0a` and `e089b5ef-6605-49b6-b93a-6ebf1e749001`. Stopped 24k coding chat: `8a229576-508c-4bfe-91ab-755dae9ca4d5`.

Final large retry: `36f138f5-84ba-4330-8f19-32e0c60ec9f5`. Fresh stopped 24k coding diagnostic: `b2ecc029-4f34-4f23-bf2e-286dde4910a6`.

Temporary audit scripts, histories, exact test configuration copies, hashes, diagnostic metadata and independent results are under `/tmp/themis-efficiency-live-20261009/`. Synthetic projects retain staged originals and immutable versioned JSONL snapshots; production persistence retains their chats. Temporary audit files are not permanent repository artifacts. All 11 chats in these nine fixture projects were verified idle. The nine exact test-only project configuration files were copied to the temporary evidence directory and removed; fixtures and saved evidence remain available. Unrelated project configurations were preserved.
