# Narrative continuity at 24k and 200k — 2026-10-10

## Scope and protocol

This synthetic live-model audit follows the [continuation fix and coding verification](continuation-loop-2026-10-09.md). It uses the same rebuilt development app and shared server, OpenCode Go / `longcat-2.5-preview-free`, and unchanged implementation `77978f9`. Executable SHA-256 was rechecked: `dc80a3f335aff51381505b54b4b60caa6a20e36c2202283b63710c1a0ff1147d`.

Two fresh projects received the same six consecutive installments of the renamed *Clarissa* narrative used in the [earlier narrative audit](narrative-compaction-six.md). Source hashes match that audit's manifest. Total input per conversation: **5,042,808 UTF-8 bytes**; each part is approximately 840 KB and exceeds the app's 200k estimated-token threshold with request overhead. This is dense prose, not repeated filler. Renaming does not eliminate possible pretraining knowledge of the recognizable story.

Project-local configuration selects **24,000 / 200,000 tokens**, two recent messages and 100 turns. Only these test chats use YOLO. No global runtime configuration, credentials, source code, system prompts or summary limits were changed for this audit. Both tests used the running app's real shared server through `target/debug/themis call`; neither used a mock provider. Runs overlapped, so elapsed times are not controlled performance comparisons.

The first five replies must be exactly `RECEIVED`, with no narrative recap that could serve as another copy. Part six requests a 1000–1400-word chronological retelling, 24 numbered answers, and four exact short quotations. Tools, archive reads and outside knowledge are forbidden in this initial phase; missing details must be marked UNKNOWN. Questions and quote cues reveal no expected answers. The 24 criteria, ten chronology relationships and four expected quotations were recorded outside the fixture projects before final answers. Actual source passages corroborate scoring; a matching name or weak search anchor alone is insufficient. The explicit full/partial/missing/error scoring policy was recorded after the all-UNKNOWN 24k answer, before either recovery answer. This is assistant-audited evidence, not an independently validated or blind benchmark.

## Initial continuity result

**Neither budget passed narrative continuity.** Both conversations naturally finished all six turns, but neither produced the requested retelling or any supported answer/quotation. Both explicitly reported that story context was unavailable instead of fabricating a narrative. A terminal `finished` event establishes completion of the run, not semantic success.

| Budget | Six-turn elapsed time | Compactions / fallback checkpoints | Supported facts | Chronology relationships established | Exact quotations |
| --- | --- | --- | --- | --- | --- |
| 24k | 307.94s | 6 / 6 | 0/24 | 0/10 | 0/4 |
| 200k | 632.42s | 6 / 6 | 0/24 | 0/10 | 0/4 |

The 24k run obeyed the no-tool condition: zero model tool calls, five exact acknowledgements, then UNKNOWN for every fact and quote. It produced no retelling. The 200k model violated the no-reread instruction once in part two with a bounded `read_file` call on that part's attachment, then acknowledged it. Thus **200k is not a pure summary-only experiment**. It still failed to provide a retelling or any supported answer at the end; it did not repeat the requested numbered UNKNOWN format. There were no operator corrections, resubmissions, observer stops or terminal `failed` events in either initial run.

## Observed mechanism and preservation

All twelve checkpoints used fallback state. Diagnostics show `output_truncated` retries and section failure before the global 240-second deadline, rather than deadline expiry. At 24k the retry allowed **512 output tokens** per section; the initial split contained 55 sections. At 200k the retry allowed **2,340 output tokens** per section; the initial split contained 14 sections. These are summarizer output limits, distinct from the configured conversation budget. The source divides a bounded working-state allowance across sections and doubles the failed request's cap once on truncation. This audit did not capture provider-private reasoning or prove why the model exceeded each output cap.

Completed section counts before failure were **[0, 1, 0, 0, 0, 0]** at 24k and **[0, 0, 0, 2, 2, 0]** at 200k. Intermediate partial notes were archived with navigation pointers; the final working state did not contain an adequate narrative. Fallback preserves current instructions and retrieval locations, which is useful for continuing a tool task, but does not supply the lost story when rereads are forbidden.

Independent checks found all six complete source texts inside each conversation's JSONL snapshots. Every original fixture and staged attachment matches its registered SHA-256. All **7 / 9** previously captured snapshots remain unchanged (24k / 200k). The original information is available on disk; this result concerns its absence from usable model context. Intact originals and safe fallback therefore do not establish narrative retention. Increasing the conversation budget alone did not make this workload pass.

## Separate recovery phase

After freezing the initial answers and scores, each existing conversation received one additional request permitting bounded local reads. It asks for a 700–1000-word retelling, the same 24 answers with source parts/anchors, and the same exact quotations. The model may decode/search originals locally with Python/bash, must stay inside its project, must not modify originals, and receives no oracle answers or corrective operator prompts. Each recovery run has a predeclared 20-minute observer limit, separate from natural completion.

**Neither recovery phase produced a final retelling.** Their semantic accuracy is unscored, rather than assigned an invented pass or a zero based on an unfinished answer. These outcomes do not change the failed initial continuity scores.

| Budget | Recovery elapsed time | Tool calls / additional compactions | Outcome |
| --- | --- | --- | --- |
| 24k | 1200.93s | 60 / 9 | Observer stopped at the predeclared 20-minute limit; no final retelling |
| 200k | 724.91s | 56 / 0 | Provider stream failed with HTTP 500 / invalid escape in tool-call arguments JSON; no final retelling |

At 24k, six of the nine additional compactions used fallback. The model retrieved facts, compacted, reread already observed details, then repeatedly searched archived search results. Its last three shell scripts scanned every snapshot, selected Tool records, searched raw `function.arguments` strings for `repentance`, `P1:` and `P2:`, and printed successive 4,000-character slices. They did not decode the tool-result JSON field. Earlier searches themselves became matching archived material. The last shell output contains nested escaped text and offload receipts, with a maximum run of **16 backslashes** even after decoding the outer shell-result JSON. Changing offsets and matching records also means this is not simply an identical-call loop caught by exact repetition detection.

Recovery generated **896,220 / 725,195 bytes** of full tool-result event strings (24k / 200k). These are cumulative event sizes, include wrappers and previously generated excerpts, and are not unique story information or model-delivered token measurements. Several 24k shell results hit the 32 KiB shell cap; large results then also encounter the shared delivery boundary. The model used local Python, but broad excerpt collection and recursive retrieval still generated substantial overhead. Bounded delivery prevents one oversized result from entering context intact; it does not prove that retrieval targets original evidence or makes semantic progress.

The 200k provider error is a separate technical failure. The stream reported malformed function-call arguments with an invalid escape; this audit does not establish responsibility within the provider/model stack. No corrective prompts, manual fixture edits or success-selecting retries were made. The 24k cancellation is the harness observer's action, although the product renders the standard `Stopped by you` text. Fresh history and `get_thread` checks confirmed both conversations idle afterward.

Independent preservation checks were repeated after both recovery runs: all six complete JSONL originals, source/staged hashes, and the earlier 7 / 9 snapshot hashes still pass. Models read root/staged story files as well as archives; this is a local source-recovery test, not proof that every recovered fact came through JSONL. Neither phase supports claiming general narrative continuity or successful autonomous recovery. The next work should address truncation-driven fallback and retrieval of generated outputs as if they were original source; raising the context budget alone does not cover the observed failures.

## Evidence and checks

- 24k chat: `cb1d0879-0c3b-4bc8-b3ba-253caf98a09a`, project `/Users/cutedandelion/ThemisOS/Projects/Narrative continuity 24k 20261010`.
- 200k chat: `39a44022-d762-41d0-bdff-f67d239fb2a5`, project `/Users/cutedandelion/ThemisOS/Projects/Narrative continuity 200k 20261010`.

Temporary evidence is under `/tmp/themis-narrative-budgets-20261010/`: manifests, rubric, quote oracle, source anchors, prompts, histories, per-turn metrics, initial scores, immutable snapshot hashes and content-free diagnostics. `run.py` orchestrates six-part sends, `recover.py` runs the distinct recovery phase, and `verify.py` checks complete JSONL originals, fixture/staged hashes, earlier snapshot hashes, acknowledgements, tool use and exact quote matches. Temporary evidence is not a permanent repository artifact.

Executed orchestration: `python3 /tmp/themis-narrative-budgets-20261010/run.py 24k` and the same command with `200k`; then `recover.py` for each budget. Independent `verify.py 24k`, `verify.py 200k`, `verify.py 24k recovery` and `verify.py 200k recovery` passed their preservation assertions. Semantic scores remain as reported above. The 24k observer initially captured the last tool event before cancellation had persisted; history was refreshed after confirming idle, and the final cancellation event was saved. No completed answer from an earlier phase was mistaken for a recovery answer.

Both initial abstentions and the recovery failure/stop states were verified visibly in the rebuilt native app after registering the existing fixture folders and reopening the frontend. Native attachment-picker interaction was not repeated; staging/sends used the same shared-server attachment path through the CLI. Test projects, full histories, source files and immutable snapshots remain available. The two exact test-only configuration files were backed up under `removed-configs/` in the evidence directory and removed after idle verification, restoring subsequent runs to normal defaults. Only normal project recency changed when opening the audit folders. The app and dashboard remain running.

Fresh `python3 -m unittest quality_dashboard.test_dashboard` passed **14 tests**; `ruff check quality_dashboard` passed. Dashboard refresh shows unchanged product metrics: 107 AST-analyzed files, mean cyclomatic **2.56**, cognitive **2.24**, maintainability **73.4**. Inline Rust tests remain in LOC/AST. Existing Rust line coverage is **20,516/23,056 (89.0%)** from the preceding coding verification; it was not regenerated for this documentation-only audit. Frontend line coverage remains unavailable. Rust/frontend suites were not rerun because product source is unchanged.
