# Live bounded-context verification — 2026-10-09

Source under test: `878ad58` on `codex/bounded-tool-context`. This is a live OpenCode Go audit using the configured `muse-spark-1.3-contributor`, separate from scripted-provider regression tests. No credential value was read or printed. All coding and attachment payloads are synthetic.

## Build and native app

Passed `npm --prefix web run build` and `cargo build -p themis-desktop --bins --features tauri/custom-protocol`. The web build retains its existing large-chunk warning. Copied the fresh executable into a development app bundle at `target/debug/bundle/macos/ThemisCode.app`, retaining the production `ai.themis.desktop` identity and resources, then signed it ad hoc locally. This is a development build, not a signed release.

Executable SHA-256: `36ca19ab84fbb835848ffb9e685b0201d09e2eed681290e3bb4b80a04ed49447`. All 44 existing chats were idle before restarting the shared backend. The new native window and backend both ran the rebuilt bundle. Existing chats were preserved; older unrelated QA backends were left running.

Native checks observed the composer send, the inventory tool/result and answer, staged attachment chip, approval denial for unrelated Playwright startup, “Compacting context…” and the final attachment answer. Other cases used the same production shared server through `themis`.

## Completed live cases

| Case | Observed result |
| --- | --- |
| Native MCP inventory | One `list_plugins` call, one installed/enabled `playwright`, no compaction; native activity showed 4 seconds. |
| Project-scoped inventory | In `Test`, one inventory call reports two enabled definitions: global `playwright`, local `browser-use`; no compaction, 8.32 seconds. Enabled is not connection health. |
| Generic large MCP result | Synthetic loopback MCP returns 1,000,066 UTF-8 text bytes. One MCP call and one Python shell retrieval return `BLOB-7493`, 10000, 59988. No compaction; 9.61 seconds. Complete original remains journaled. Python output was 2,410 bytes and included a bounded marker excerpt. |
| Small coding/JSONL pipeline | Seeded synthetic history contains an earlier requirement and a later correction. One forced compaction, seven tool calls, 28.31 seconds. Model fixes decimal/per-row rounding and status filtering; independent six-test suite passes. All 10,000 output records match the independent transformation oracle; sum is 59988. Tests and preservation sentinel remain unchanged. This is a plumbing test, not a difficult coding benchmark. |
| Native text attachment | 186,117-byte text, 32,000-token project budget, one forced compaction, no model tool calls. All beginning/middle/end marker/name/amount values are correct, and all nine values occur in the checkpoint. |
| Recurrent attachments | A second 186,115-byte text plus an 8,192-byte binary attachment force another compaction in the same chat. Three new records and the earlier middle record are all correct, without model tool calls; 20.32 seconds after startup approval. Both staged copies are byte-identical. |

The MCP scope discrepancy was real scope behavior, not deletion or hidden pagination: `browser-use` is saved only in `/Users/cutedandelion/ThemisOS/Projects/Test/.themis/plugins/registry.json`; global `playwright` is available to the new verification project. Both the registry metadata API and live model agree. Its enabled flag does not establish that `browser-use` connected successfully.

## Large multi-file coding stress

The synthetic settlement engine has eight Python modules. Bugs span parser validation/canonicalization, global revision deduplication, status filtering, per-currency/per-row rounding, zero totals/sorting, and CLI error behavior. Four modules contain large immutable reference tuples; the size padding is synthetic, and difficulty comes from the interacting contracts, not LOC alone. Tests must remain unchanged and reference tuples must remain value-identical. The contract includes conflicting obsolete revisions, newer pending versions suppressing older posted ones, idempotent canonical retries, refunds, JPY, exact large decimal amounts, and invalid JSON/input shapes.

Baseline: 16 test methods run with 20 subtest failures and four errors. A separate held-out oracle was written before examining the model's changes: 200 randomized multi-revision ledgers, precision/context checks, validation and CLI shape cases, plus table/test integrity checks.

**24,000-token stress case failed to make progress.** With 543,325 attached bytes, the model repeatedly inspected requirements and implementation without editing. It was explicitly stopped after 339.53 seconds, 25 tool calls, seven compaction starts and six completed checkpoints. The seventh compaction was cancelled. The terminal failure is the operator Stop, not a provider error. Source files remained unchanged. This is evidence that bounded output and the archive-recovery guard do not eliminate repeated ordinary-file inspection or excessive compaction under a small budget. Do not report this case as a pass.

The 200,000-token Muse case with 1,623,238 attached bytes was blocked by OpenCode Go's HTTP 429 weekly usage limit after 86.32 seconds. It made zero tool calls/edits. One compaction started and saved a recovery fallback checkpoint (not a successful working summary); complete originals and partial section notes remain available. This case is unverified for coding success. A listed free model, `longcat-2.5-preview-free`, successfully returned READY through the same provider and was evaluated separately, as described next. Input size, rather than shrinking normal workspace settings, forces its compaction. These observations are not a controlled model-performance comparison.

## Completed large-source free-model result

`longcat-2.5-preview-free` finished naturally in **774.14 seconds (12m54s)** with **82 tool calls** against 1,623,359 attached bytes and the normal 200,000-token budget. One forced compaction exceeded the 240-second deadline and saved a 4,663-character recovery checkpoint; this is **fallback recovery**, not successful summary retention. The continuing model inspected current files, repaired six modules and finished with a final result. There was no further compaction in this run.

Independent checks after the terminal `finished` event:

- `python3 -m unittest discover -s tests -v`, from the large coding project: all 16 methods pass.
- `python3 /tmp/themis-bounded-live-20261009/large_free_oracle.py`: all 210 held-out checks pass, including 200 randomized ledgers, 40-digit precision/global-context preservation, invalid field types and CLI shapes.
- All four reference tuple blocks are byte-identical to staged originals; their values are also identical. The supplied test file hash is unchanged.
- All 12 pre-edit journal hashes remain unchanged. Text/binary staged copies remain identical after their original source paths were moved.

Execution was inefficient: 3 native reads, 3 directory listings, 40 shell calls, 34 patch calls and 2 writes; 21 tool returns failed during patch attempts. The model eventually recovered from those errors. Its additional reported 32 edge checks, 6,000 differential cases and 40,000 malformed-input fuzz cases are model-run checks, distinct from the independent 210-case oracle. The fixture's large reference tables are generated padding; this is a multi-module contract/recovery test, not evidence of performance on a real complex application.

The code repair passes this synthetic acceptance suite. The low-budget compaction/read loop remains an unresolved finding, and the large-source free-model summarizer timed out. Therefore this audit does **not** establish that repeated compaction or agent loops have been eliminated generally. No product code was changed to conceal either finding. The synthetic MCP was disabled after its test; production default model/budget and the user's existing integration scopes were preserved. Test-only runtime overrides were removed after the audit so these projects inherit normal defaults; their exact forced-compaction settings were saved in the temporary audit artifacts. No additional restart-recovery test was performed during this follow-up audit.

## Fresh automated checks and code health

- `cargo test -p themis-desktop --test attachments_e2e --test server_cli_e2e`: 19 passed, zero failed/ignored/filtered.
- `python3 -m unittest quality_dashboard.test_dashboard`: 14 passed.
- `ruff check quality_dashboard`: passed.
- Dashboard refreshed at `2026-10-09T18:35:34Z`: 107 product files, mean cyclomatic 2.56, cognitive 2.24, maintainability 73.5. Rust coverage shown is the existing 19,745/22,269 (88.7%) artifact from the implementation verification, not regenerated for this audit. No product source changed in this audit; frontend coverage remains unavailable.

The earlier complete lint/workspace/frontend/coverage checks are in [bounded tool context](bounded-tool-context.md). They are historical relative to these live observations; the fresh commands above are the checks rerun here.

## Durable evidence

Production `sessions.sqlite3` retains these test chats:

- Native inventory: `21d323a3-2f5c-4c87-b2ba-28aec65b58d1`.
- Scoped inventory: `efadc9ed-110b-4af3-be6c-7b31f88d24f9`.
- Generic large MCP: `bb83a6ba-b9bb-414b-9531-1e45fc57ad4f`.
- Small coding: `fef8c74d-0ab8-4fcd-846f-41a7152e4359`.
- Recurrent attachments: `5618e294-802b-4fdf-aebc-348f466e68ad`.
- 24k multi-file stress: `d089ec19-670e-4a25-9742-f727aea8b607`.
- Normal-budget Muse quota failure: `b8f49bef-90a5-49d9-a43b-74192cc957a3`.
- Completed free-model large coding: `d2c9c7a9-23ad-478b-92e8-f179afcdc81a`.

Local audit scripts, independently evaluated fixtures/oracles, scoped histories, hashes and logs are under `/tmp/themis-bounded-live-20261009/`; those temporary files are not permanent repository artifacts. Test projects retain their staged attachments and JSONL journals. The test MCP is local and synthetic; autonomous choice of its tool/retrieval comes from the real provider, not a scripted model.
