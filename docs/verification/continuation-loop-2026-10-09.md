# Continuation recovery verification — 2026-10-09

This continues `codex/bounded-tool-context` and the [preceding audit](context-efficiency-2026-10-09.md). Live-model runs are synthetic audits, separate from deterministic regression checks.

## Shared change

Compaction previously cleared the public narration beside a restored completed invocation. A prepared repair script could therefore retain its write receipt while losing the pending execution/verification intent, especially when a lossy summary still recommended inspection. The shared runtime now retains up to 512 characters of public intent both beside the latest restored invocation and in bounded historical action records. Private reasoning is not captured. Observed actions supersede stale summary plans; a saved artifact does not imply execution or correctness.

Prior task narrative feeds summary updates without recursively resummarizing generated cache/recovery envelopes. Complete originals and section dump coordinates remain unchanged. Cache assembly targets headroom of half the message allowance beyond instructions/current request, or 20% overall, whichever is larger. Mandatory state can exceed that target and is preserved. Guidance asks for checking source affected by later changes rather than restarting inspection solely because compaction occurred.

This uses existing checkpoint, action-cache, archive and restart paths. No additional plan store, tool-specific workaround or semantic-loop detector was added. Review found no actionable issue, including in permission handling, public-text scope, original coordinates and the headroom floor.

## Regression evidence

The public-intent retention and generated-envelope tests failed before the shared fixes. The CLI regression also failed with HTTP 422 from its test provider because the actual continuation request lost the prepared action intent. It uses real CLI/shared-server persistence, compaction, file write and Python execution, with a deterministic provider that deliberately returns a stale summary. The initial CLI fixture omitted the required `append` write argument; this was corrected before green acceptance, with successful-write, execution-exit and actual output-file assertions retained. An intervening wider run caught the expected headroom assertion and an obsolete checkpoint-wrapper expectation; both were resolved without dropping retained-state checks.

## Rebuild and automated checks

`npm --prefix web run build` and `cargo build -p themis-desktop --bins --features tauri/custom-protocol` passed. The bundled executable was replaced atomically, signed ad hoc, and verified with `codesign --verify --deep --strict`. It is a development build. Executable SHA-256: `dc80a3f335aff51381505b54b4b60caa6a20e36c2202283b63710c1a0ff1147d`. All 62 persisted chats were idle before restart. The new bundled backend and native UI launched successfully; the UI loaded `tauri://localhost` and authenticated Go availability was confirmed without reading or printing credentials. Unrelated QA backends remained running.

- `cargo test -p themis-core --lib checkpoint_ -- --nocapture`: seven passed.
- `cargo test -p themis-desktop --test attachments_e2e`: seven passed, including the constructed stale-summary continuation request, real script execution, recurrent recovery/restart, bounded MCP results and original preservation.
- `cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten --test-threads=1`: 362 passed, two opt-in tests and one documentation example ignored, one host Keychain test explicitly excluded.
- `cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten`: 362 passed, two opt-in tests ignored, one host Keychain test explicitly excluded. Fresh Rust line coverage: **20,516/23,056 (89.0%)**. This does not establish Keychain persistence or frontend line coverage.
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `git diff --check`: passed.
- `npm --prefix web test`: 306 tests across 29 files passed. Web lint and build passed; existing Vite chunk-size and Vitest deprecation warnings remain.
- `python3 -m unittest quality_dashboard.test_dashboard`: 14 passed. `ruff check quality_dashboard`: passed.

Two ordinary parallel workspace attempts stalled in the existing `agent_can_disable_broken_enabled_mcp_without_losing_chat` approval flow and were stopped. The cause is unconfirmed. `cargo test -p themis-desktop --test plugins_e2e agent_can_disable_broken_enabled_mcp_without_losing_chat -- --nocapture` passed in isolation; the full serial workspace run and parallel coverage run also passed that test. No source or test changes were made for the stall. Logs are preserved at `/tmp/themis-continuation-workspace-serial.log`, `/tmp/themis-continuation-mcp-stall-probe.log` and `/tmp/themis-continuation-coverage-verified.log`; this is not a claim that the ordinary parallel run passed.

The refreshed dashboard analyzes 107 product files with rust-code-analysis 0.0.25. Mean cyclomatic/cognitive/maintainability: **2.56 / 2.24 / 73.4**. Runtime: **4,433 LOC, 2.56 / 2.28 / 68.3**, compared with 4,388 LOC and 2.57 / 2.27 / 68.4 in the preceding audit. Inline Rust tests count in LOC/AST; the larger runtime and slightly lower maintainability remain a maintenance concentration, not a claimed metric improvement. Frontend line coverage is unavailable. Product automated inventory is 611 tests, separate from 14 dashboard self-tests; discovered inventory is not execution or coverage.

## Live audit protocol

The two fresh projects use identical **1,623,145-byte** attachments, eight Python modules, four large immutable reference tables and six interacting broken modules. Both baselines fail the supplied 16-test suite. Explicit project configuration sets 24,000 or 200,000 tokens, two recent messages and 100 turns. The request scopes all work to its own project and forbids searching the home directory/other projects. Live runs use Longcat through the rebuilt app's shared server and existing Go authentication, with test-chat YOLO only. A 25-minute observer limit is distinguished from natural completion/failure.

Independent acceptance reruns the supplied suite and the preprepared 210-check oracle (200 randomized ledgers plus large precision, global Decimal-context, invalid-type and CLI checks), verifies exact reference-block bytes and supplied-test/requirement hashes, stdlib imports, complete staged originals and previously captured snapshot hashes. The oracle does not cover every possible amount-string lexical form. These are synthetic functional audits, not controlled performance measurements or proof against every possible model loop.

## Live acceptance

| Configured budget | Natural completion | Tool calls / compactions | Independent checks |
| --- | --- | --- | --- |
| 24,000 | 755.04s | 34 / 5 | 16 supplied + 210 oracle + 16 model edge checks passed |
| 200,000 | 362.82s | 39 / 1 | 16 supplied + 210 oracle + 120 model edge checks passed |

Both chats were observed completed in the rebuilt native UI. Six source modules were repaired in each project. All four reference-table blocks are byte-identical; supplied tests and requirements are unchanged; imports remain stdlib only. All eight full attachments occur in each initial snapshot, staged copies match their pre-edit hashes, and all **39 / 41** previously captured snapshot hashes remain unchanged (24k / 200k respectively). The 24k model removed its scratch scripts; its edge-check file was recovered from original archived invocation arguments and independently rerun successfully. Its final answer claimed 17 edge checks, while the recovered script actually reports 16; this audit uses the observed count. The 200k model's retained scripts independently report 108 general plus 12 CLI checks.

At 24k, the critical transition now continued: the prepared repair script survived the second checkpoint and was executed, followed by the supplied suite, additional checks, corrections to the model's own test expectations/quoting, and natural completion. There was still unnecessary rereading, including an oversized table-prefix shell read. This was not an endless continuation loop, but it was not efficient: compactions consumed about **417.9s** in total (23.325s and 127.292s fallback; then 121.234s, 48.402s and 97.678s successful updates). Small output caps caused truncation retries. No checkpoint-budget failure occurred.

At 200k, the initial attachment compaction used fallback after **138.507s**, retaining six completed section notes. Subsequent coding and verification finished naturally without another compaction. The live runs overlapped after the 200k run started; latency is not a controlled comparison. Neither run received corrective operator prompts or fixture edits. Model-generated checks had some failures before self-correction; the independent final checks all passed.

The demonstrated continuation-loss fix is shared and covered through the actual request/restart paths. These passes do not guarantee that any model can never repeat semantically similar work. The 24k setting still incurs frequent, expensive compaction for this fixture. No universal semantic-progress gate or model-independent correctness claim follows from this audit.

## Evidence and cleanup

Implementation commit: `77978f9` (`fix: retain pending action intent through compaction`).

- 24k chat: `e29a1f04-0d12-463e-84ec-5b2e1673842e`, project `/Users/cutedandelion/ThemisOS/Projects/Continuation 24k coding 20261009`.
- 200k chat: `1dcc7795-91b7-4190-ae23-7e2863df152b`, project `/Users/cutedandelion/ThemisOS/Projects/Continuation 200k coding 20261009`.

Temporary harnesses, full histories, model/independent check results, build hash, baseline/snapshot hashes and content-free compaction metadata are under `/tmp/themis-continuation-live-20261009/`. These temporary files are not permanent repository artifacts. Both chats were verified idle. Their two exact test-only project configuration files were backed up there and removed; fixture projects, staged originals, immutable snapshots and saved chats remain. Both snapshot observers were stopped. Original user-stopped chats were left stopped. The rebuilt app and dashboard remain available.
