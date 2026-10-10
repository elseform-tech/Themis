# Document preparation deadlines and resumable cache identity

## Trigger and failure

The one-chat 200k narrative trial stopped during preparation, before ordinary generation: three of six complete story documents had durable capsules; the fourth had one completed stage. The next stage exhausted three HTTP 429 attempts. The upstream returned endpoint-unavailable errors; this does not establish an account quota issue. The old 900-second deadline covered all six documents. The first three used approximately 811 seconds, leaving little time for the fourth. The final retries waited 1.077 and 2.569 seconds.

Evidence is retained under `/Users/cutedandelion/ThemisOS/Verification/compaction-20261010/adaptive-context/single-thread-v1/200k-narrative/`. Its `failure-metrics.json` records 25 summary attempts, eight automatic retries, 16 completed stages, zero ordinary requests/tools/checkpoints, 770,741 reported input tokens and 30,392 reported output tokens. Nine attempts have no provider token accounting. This failed work remains part of the journey cost; it is not a completed task or a zero-cost setup run.

## Change

`compaction.timeout_seconds` bounds each uncached document or context checkpoint across all stages, retries and waits. `compaction.document_batch_timeout_seconds` separately bounds complete attachment preparation, default 3600 seconds and validated at 1–21600. Each fresh document deadline is capped by the remaining batch time. Request timeout, Stop and the two-retry maximum remain nested bounds.

Summary 429 retries use `compaction.rate_limit_backoff_seconds`, default ten seconds then twenty, with jitter up to half the base and Retry-After as a floor. The field accepts 1–300. Other transient errors and ordinary answer backoff retain their existing one-/two-second bases. Waiting longer may help transient recovery; it does not guarantee endpoint availability.

Execution-only changes no longer invalidate completed summaries. Semantic keys bind resolved prompts, source, purpose, model, input/output budgets and pipeline. Exact old typed serialization supports known legacy identities without scanning or guessing historical settings. Legacy stages undergo the same bounded regular-file/schema/input/text validation before create-new promotion; originals remain unchanged. Capsule migration verifies path, hash, bytes, pipeline, model, policy and validated original snapshot. Custom resolved prompts disable ambiguous legacy fallback. Stop wins before migration publication and before returning a cached batch.

Configuration uses existing JSONC resolution/provenance and captured run snapshots. Rotating JSONL diagnostics record limits, remaining batch time, actual waits, migration/reuse and request usage, without document contents or provider bodies. There is no new registry, configuration UI or manual compaction-retry command.

## Verification

The old implementation failed the fresh-document-deadline and summary-429-floor regressions. The initial patched core run passed 190 tests. The final patched core run passed 191 tests; the fresh full workspace run is still in progress. The reviewer cleared the final source diff, including cache preference and Stop publication fixes. Rebuild/native and live rerun evidence remain separate gates.

The first unfiltered workspace run reached 102 passing desktop unit tests and one failure: the existing cold-catalog Stop test's 20-second pre-request setup allowance expired under parallel load, before its cancellation assertions. The isolated reproduction passed in 15.33 seconds. Preparation includes filesystem skill discovery, but its exact latency contribution was not instrumented. The fixture now allows 60 seconds for admission and reports early send errors immediately; the two-second Stop check remains unchanged. The original failure log is retained. A fresh workspace run excludes the OS Keychain persistence test to avoid another credential-store interaction; that test passed in the original unfiltered run. The fresh desktop unit run passed 102 tests with that one named OS test filtered out; final workspace acceptance remains pending.

Focused checks cover independent document job deadlines, outer batch cutoff with completed work retained, cancellation during rate-limit waits, exact legacy serialization, stage/capsule promotion without provider calls, reuse after execution-setting changes, and semantic incompatibility. Shared-server CLI configuration checks verify new values, provenance and restart persistence.

Fresh focused checks passed: six shared-server/CLI configuration tests; workspace Clippy with warnings denied; rustfmt and diff checks; frontend lint, 306 tests across 29 files and production build; Ruff and 14 dashboard tests. Available Rust line coverage is stale (89.1%, 21,023/23,586 lines) and predates this patch; frontend coverage is unavailable. The refreshed health inventory includes 114 app files, mean cyclomatic complexity 2.58, cognitive complexity 2.21 and maintainability 73.2, with inline Rust tests included. These are inventory means, not acceptance thresholds.

Native verification caught a packaging failure: a default/test binary overwrote the custom-protocol build before copying, so the launched bundle showed a blank frontend while the shared backend restored the chat and captured configuration correctly. No live trial ran in that bundle. The staged rebuild now must verify that both current main JS/CSS asset names are embedded in the copied binary before replacement, followed by strict signature verification and visible native proof.

The technical live rerun must use the same durable chat, identical question bytes and existing staged attachments, retain the original failure/cost, and supply no semantic answer feedback. It must demonstrate reuse of the three completed capsules and fourth document stage before resuming unfinished work. Factual and source grading remain separate from structural summary acceptance.

## Efficiency evidence and limits

One earlier genuine 200k checkpoint reduced estimated message context from 172,870 to 42,986 tokens (75.13%; 129,884 freed; 4.02x compression). Including unchanged 7,819-token tool schemas, estimated input fell from 180,689 to 50,805 (71.88%). It took 89,719 ms across four successful stages and one automatic retry. Successful stages reported 265,967 input and 1,710 output tokens; failed-attempt usage was unavailable. These are context-occupancy measurements, not billed-token savings or factual-retention percentages. The 256k/24k acceptance matrix and autonomous coding/continuity acceptance are still pending.
