# Adaptive context management implementation and verification

Status: in progress; no live success claimed. Existing branch: `codex/bounded-tool-context`.

## Authorized scope

One adaptive pipeline for 24,000, 200,000 and 256,000 tokens: reversible maintenance before semantic compaction; durable original evidence; active/latest-task continuity, older-task and discussion recall; bounded staged summaries with atomic installation and no degraded continuation; shared validated configuration; metadata-only context distribution and provider usage telemetry; durable run aggregates for a future dashboard.

## Work sequence

1. Repair the inherited action-cache regression and preserve its original red evidence.
2. Add configuration/schema/256k limits and focused boundary tests.
3. Implement context maintenance, staged resumable compaction and failure boundaries with runtime tests.
4. Implement durable task/discussion retrieval and bounded document admission through the shared server.
5. Add distribution/usage telemetry, persistence and query support; document measurement provenance.
6. Run relevant and workspace checks, dashboard metrics and native rebuild verification.
7. Run live cases sequentially at 24k, then 200k, then 256k: coding, narrative, latest follow-ups, older-task resumption and topic recall. Stop on the first material issue, preserve evidence, add a regression and fix it before proceeding.

## Evaluation requirements

Separate natural from forced compaction; mock/server/CLI/native/live evidence; matched tasks from pressure variants; cold document preparation from reuse. Use the selected Step 5 Preview Free coding model and LongCat narrative model, with nonthinking LongCat internal summaries. Verify provider capacity rather than relabeling a reduced budget as 256k. Do not change credentials or integration registries.

Track correctness and autonomous completion, factual/chronology/quotation accuracy, latest instruction adherence, task contamination and freshness, discussion decision status, total latency/tokens/cost including failures, recovery and repeated work, compaction frequency and productive headroom. Missing usage remains unavailable. Diagnostic logs exclude content; originals remain in transcript/evidence storage.

## Evidence

Inherited selected suite: 149 core tests passed, one failed (`checkpoint_jointly_budgets_caches_and_archived_completed_arguments`); source `/Users/cutedandelion/ThemisOS/Verification/compaction-20261010/small-action-cache-tests.log`. The unfinished cache patch displaced the newest read coordinate under tight joint budgets. The focused runtime suite verified the repair: newest evidence locations survive the joint cache budget and subsequent checkpoints. Later changes require the fresh full checks below.

## Reviewer pass and subsequent fixes

A separate read-only reviewer identified message-per-stage overflow, premature deadline slicing, production retry reconstruction gaps, assistant-history evidence misclassification, and acceptance of nonempty malformed summaries. Patches pack records, share an absolute stage deadline across retries, persist resumable manifests with stable IDs, classify original exchanges distinctly, and validate ordered nonempty checkpoint sections. The added server/CLI retry regression first exposed a real debug-runtime stack overflow; boxing large async futures removed it. A subsequent fixture error (missing mandatory `usage.total_tokens`) was isolated and repaired. Fresh verification is still in progress; this is not a live acceptance claim.

The reviewer also identified a full-document narrative-preparation gap: attachment maps are bounded navigation and do not prove complete semantic processing. That remains an explicit unresolved acceptance item before narrative success can be claimed.


## Fresh automated progress

- Core library: 164 passed in `/tmp/themis-adaptive-workspace-5.log`. Subsequent worst-case JSON escaping/cache reuse regression passed in `/tmp/themis-stage-escape-green.log`. The entire retry/backoff operation obeys the stage deadline. Symlink caches are rejected, unique stage temporaries preserve originals, and serialized cache reads are bounded with sixfold escape expansion accounted for.
- Core runtime integration: 20 passed, one live opt-in ignored (`/tmp/themis-adaptive-runtime-5.log`). Maintenance avoids unnecessary summaries, real semantic pressure still compacts, and no completed read is replayed.
- Shared server/CLI durable retry: passed (`/tmp/themis-durable-retry-6.log`), including stage reuse and run-filtered usage readback.
- Web: lint passed, 306 tests passed, build passed; existing large-chunk warning remains. Dashboard: Ruff and all 14 tests passed. These are local checks, not native or live-provider acceptance.
- Attachment integration: all 12 passed (`/tmp/themis-attachments-adaptive-11.log`). Legacy import pressure uses valid bounded messages. Restart recovery discovers the original snapshot through model-visible `list_dir` output. Historical expectations about eager full-file admission, mandatory end-turn summaries, sampled middle omission and a fixed embedded Python recipe were migrated. Full-document narrative acceptance is still unresolved.
- Desktop library: all 102 passed, including the unfiltered Keychain test. Full workspace run stopped at a bridge test that indexed operational events without filtering new ContextUsage metadata; the focused fix passed all seven bridge tests (`/tmp/themis-adaptive-bridge-6.log`). The remaining desktop targets passed across `/tmp/themis-adaptive-desktop-remaining-6.log` and `/tmp/themis-adaptive-desktop-remaining-7.log`; command coverage required an allowlist update for the advanced-only usage API. Do not report the original full workspace command as passing.
- Full workspace Clippy passed again after the observer migration (`/tmp/themis-adaptive-clippy-6.log`). Format and diff checks passed. Existing coverage is stale and does not certify this patch.

## Rebuilt app and first live trial

Development custom-protocol build and web production build passed. The existing app bundle was refreshed and ad-hoc signed, then relaunched with verified source/bundle hashes and old frontend PID absence. Fresh frontend/server PIDs were 55012/55057. Native window and Advanced context settings were observed. This is a development bundle, not a release build. Evidence: `adaptive-context/build.json` and `native-readiness.json` under the external verification directory. Use the bundled CLI during live trials because concurrent workspace test builds can replace target/debug binaries.

The first 24k coding trial was intentionally stopped at 81.26 seconds when the independent workspace bridge observer mismatch surfaced. It completed four reducing maintenance passes and one first-attempt nonthinking LongCat summary (21.234 seconds, 8,833 reported input tokens and 622 output tokens), then continued tool execution. Twelve tools ran. This is partial diagnostic evidence, not coding correctness or completed live acceptance. The second 24k trial was also intentionally stopped when the independent command-coverage allowlist mismatch surfaced. It completed two checkpoints, both from successful first-attempt nonthinking LongCat calls (22.721 and 25.996 seconds). The endpoint was working; the test allowlist needed to classify it as advanced-only. These three successful summary calls across partial trials do not establish a reliable failure rate or semantic fidelity. No live coding case has completed, and no larger-budget test has begun. A clean retry is prepared in `adaptive-context/coding-retry2`. Starting live trials before all contract checks completed caused avoidable interruptions; subsequent trials wait for checks and a frozen rebuilt bundle.

## Durable recovery error correction

The reviewer found that storage failures advertised retry commands before a resumable job existed. Both the inner compaction failure and the outer runtime wrapper reproduced the error in RED regressions (`/tmp/themis-pending-storage-red.log`, `/tmp/themis-pending-wrapper-red.log`). The fix tracks original-evidence publication separately from ID-manifest publication and the latest-job pointer. An explicit retry ID is offered only after that manifest is saved; pointer-only failure leaves the ID usable. Owned publication temporaries are cleaned up on error. Core library verification passed 165 tests (`/tmp/themis-pending-storage-green.log`); runtime integration passed 21 with one opt-in live test ignored (`/tmp/themis-pending-runtime-green.log`). Shared-server recovery checks passed all 18 tests (`/tmp/themis-pending-shared-green.log`), workspace Clippy passed (`/tmp/themis-pending-clippy.log`), and the no-evidence-directory regression passed (`/tmp/themis-pending-no-directory-green.log`). Workspace doctests completed with zero executable tests and one ignored example (`/tmp/themis-adaptive-doctests.log`). Commit `0bba1c6` contains the focused correction. No native rebuild of this correction occurred before the next user steering. Reviewer found no blocking issue in this focused patch; review was read-only.

## Automatic retry steering

The user requested no exposed retry-compaction command and explicitly prioritized reliable completion over summary speed. The current patch removes slash-command handling across core/desktop, defaults to two automatic retries per failed stage, keeps retry output limits at base/2x/2x, applies structural validation to every attempt and shares the absolute stage/job deadlines across backoff and requests. Overall timeout defaults to 900 seconds with a validated 1800-second ceiling; individual requests remain capped at 300 seconds. Explicit existing overrides remain valid. The new third-attempt regression reproduced the old two-attempt stop (`/tmp/themis-automatic-compaction-red.log`). Initial checks found an existing deadline-message assertion that assumed which nested timeout guard fired first; its strict elapsed-time/request-count/cache checks remain intact, with either timeout owner accepted. Verification and reviewer inspection are in progress. No further live trial has started.

## Automatic retry verification

Fresh core library verification passed all 167 tests (`/tmp/themis-automatic-compaction-core-final.log`); its runtime target initially exposed an invalid new pressure fixture, which maintenance correctly reduced without summarizing. The corrected fixture retains semantic history and bounded narration, and verifies that every summary attempt observes the already-completed append. The full runtime target then passed 23 tests with one opt-in live test ignored (`/tmp/themis-automatic-compaction-runtime-order-final.log`); two 503 failures recover on attempt 3, exactly one write and one checkpoint occur, and ordinary generation finishes. The earlier missing evidence directory was also a fixture error, not a product failure.

The final shared-server run passed 12 attachment, 6 configuration/diagnostics and 13 CLI tests (`/tmp/themis-automatic-compaction-shared-final.log`). Its single-request automatic retry case confirms completed stage reuse, attempt 2 telemetry, no pending manifest after success, and honest missing usage for both 503 responses. Workspace Clippy passed (`/tmp/themis-automatic-compaction-clippy-final.log`); formatting/diff checks passed. Dashboard Ruff and 14 self-tests passed (`/tmp/themis-automatic-compaction-dashboard-tests.log`). These are mock/local checks, not completed live acceptance.

The read-only reviewer found no blocking automatic retry defect and identified the obsolete latest-job pointer as an unnecessary failure dependency. Production no longer writes or consumes it; legacy files remain untouched. The reviewer also checked the append-order proof. Updated dashboard metrics (`adaptive-context/dashboard-metrics-automatic-retry-final.json`) cover 112 files: mean cyclomatic 2.56, cognitive 2.20, maintainability 73.3 (previous refresh 73.4). Rust inline tests remain included. Existing 89.1% line coverage is stale. Full-document capsules and the complete live matrix remain outstanding; no narrative continuity claim is made.
