# Simplify compaction (2026-10-15)

Roadmap item 5. Three kinds of evidence, kept apart:

- **Automated checks (CI-safe, offline, scripted providers):** prove the wiring, not model quality.
- **Benchmark audit (live model, not in CI):** `longcat-2.5-preview-free` (OpenCode Go,
  non-thinking), 24k budget, 3 trials per arm, isolated `HOME` per run, same day. One free model;
  SDs of 2-20 points, so differences below about 5 points (compaction suite) or 15 points
  (long-form) are noise.
- **Live end-to-end run:** the built `themis` CLI against the shared server and the free model,
  isolated `HOME` and data directory.

## Layer inventory

Production lines are from `main` (c34da50); tests excluded.

| Layer | Where | Trigger | What it does | LOC (main) | After this branch |
| --- | --- | --- | --- | --- | --- |
| Budget and triggers | `runtime.rs` run loop | every request | byte estimate x provider calibration, thresholds (maintenance 0.83, checkpoint 0.93 minus a growth forecast), short-reply deferral, overflow recovery, `context_budget` event | ~200 | unchanged |
| Token calibration | `calibration.rs` | provider usage | per-run ratio, overflow classification | 252 | unchanged |
| Maintenance | `runtime/maintenance.rs` | >= 0.83 | snapshot, then receipts for completed invocation arguments, recovery copies and long public intent; ordinary observations protected until summarized | 134 | unchanged |
| Checkpoint | `runtime.rs` (`compact_context*`, `prepare_compaction`, `replace_context`, archive) | >= threshold, overflow, end-of-run handoff | save originals snapshot, summarize, assemble system + active request + checkpoint + recent tail | ~1,000 | **one summary request per checkpoint**; excerpt/oldest-record cut with coverage notice |
| Staged summary jobs | `runtime/summary_job.rs` | context packet or document did not fit | fragments, up to 32 sequential stages + consolidation, stage cache, legacy key migration | ~595 | **documents only**; context wrapper removed |
| Summary projection | `runtime/summary_evidence.rs` | every summary | provenance projection, recovery exclusion, original-derived excerpts, maintenance protection | 330 | unchanged |
| Pending job inputs | `runtime/pending.rs` | every context checkpoint + documents | wrote a second full copy of the context job input (never read outside tests) | 127 | **context copy removed** (50 lines, documents only) |
| Suspended progress | `runtime.rs::suspended_progress` | noncancelled checkpoint failure | bounded ledger record for the next run | ~36 | kept; now gated on saved originals only |
| Task ledger | `runtime/ledger.rs` | checkpoint assembly | plan, completed actions, read excerpts/locations | 398 | unchanged |
| Document preparation | `runtime/documents.rs` | attached `.txt`/`.md` | capsules via the staged pipeline, per-document and batch deadlines | 238 | unchanged |
| Recovery text | `runtime.rs` system text + checkpoint navigation | every request | JSONL/Python recovery recipe + recall pointer | ~4.0 KB/request | **3.4 KB/request**; recipe only in the read_file fallback |
| Recall | `tools/recall.rs` | model tool call | bounded search over the chat's originals | 1,364 | **inverse-document-frequency ranking** (+55) |
| Social detection / anchors | `social.rs`, desktop transcript | each run | short-reply deferral, no anchors for greetings | 376 | unchanged |

Net production change in `crates/core/src`: see the metrics table below. The simplification
is mostly behavioral (one request per checkpoint, one copy of each job input, a shorter system
text); the larger removals were not made, for the reasons in "Not changed".

## What changed

1. **One summary request per context checkpoint** (`feat(runtime): one summary request per
   context checkpoint`). Long fields keep a head/tail excerpt with UTF-8 byte coordinates; when
   even empty fields cannot fit, the oldest new records are left out. The request carries a
   coverage notice, a cut requires saved originals, and `recall` reaches what was cut. Attached
   documents keep the staged pipeline (AGENTS.md per-document/batch deadline rules unchanged).
2. **No duplicate pending copy** of the context job input: the saved snapshot is the input.
   Suspended progress is published when originals were saved.
3. **Shorter shared system text** and a recall-first pointer: greetings, thanks and small talk get
   a reply in kind without recapping parked work; the checkpoint is described as a lossy
   paraphrase; recall queries should be 2-5 distinctive keywords; the JSONL/Python recovery recipe
   moved to the read_file fallback used only when recall is not offered.
4. **Recall ranking** weights terms by inverse document frequency, so a rare name outranks
   several common words when the model passes the whole question.
5. **Eval harness:** truncated probe answers retry once with a larger cap; remaining probe request
   errors are excluded from retention (counted under `probe_errors`); behavior turns offer
   `recall` as the app does.

## tool_result investigation

The 2026-10-14 drop (0.81 to 0.67) was mostly probe request failures, not lost facts: 4 of the 7
non-correct tool_result answers were `Provider response was truncated` errors of the 400-token
probe answer, scored as failures. With one retry at 1,600 tokens and errors excluded, main
measures 0.857 +- 0.143 and this branch 0.905 +- 0.083 (no probe errors on main, 2 on the final
branch run, from HTTP 429 under parallel load).

## Ablations (benchmark audit)

Toggles: `docs/verification/simplify-compaction-2026-10-15/ablation-toggles.patch` (environment
variable `THEMIS_ABLATE`, applied to an uncommitted build of the harness commit fd6fbed; not in
this branch). `main` is c34da50 plus the harness commit (eval code only).

Compaction suite (`python3 -m evals.compaction run --suite core --trials 3 --budget 24000`,
summary-only retention unless noted):

| Metric | main | single pass | lean checkpoint prompt |
| --- | --- | --- | --- |
| Summary-only retention | 0.927 +- 0.054 | 0.946 +- 0.018 | 0.939 +- 0.028 |
| Overall retention | 0.949 +- 0.038 | 0.958 +- 0.015 | 0.953 +- 0.027 |
| parked_topic | 0.976 +- 0.041 | 0.976 +- 0.041 | 0.905 +- 0.041 |
| decision | 0.556 +- 0.193 | 1.000 +- 0.000 | 0.778 +- 0.193 |
| tool_result | 0.857 +- 0.143 | 0.905 +- 0.083 | 1.000 +- 0.000 |
| middle position | 0.978 +- 0.038 | 0.911 +- 0.038 | 0.889 +- 0.077 |
| Requests per checkpoint | 2.04 | **1.00** | 1.93 |
| Checkpoint latency | 33.1 s | **18.9 s** | 30.8 s |
| Wall time (3 trials, 3 jobs) | 20.7 min | 16.8 min | 20.1 min |

Long-form, single pass on the only cases where the context path staged (`meeting`, `build-log`;
the other four cases always fit one request), same cases from the main run:

| Metric | main (2 cases) | single pass |
| --- | --- | --- |
| Correct with valid citation (all) | 33.9% +- 9.2 | 31.1% +- 11.7 |
| Correct, no citation requirement | 45.0% +- 8.8 | 38.5% +- 13.6 |
| Input tokens / probe | 103,047 | 91,022 |
| Latency / probe | 73.4 s | 53.7 s |

Decisions:

- **Single pass adopted.** Retention is within noise of staging at half the requests and about
  43% less checkpoint latency; long-form accuracy on the staged cases is within noise.
- **Lean checkpoint prompt not adopted.** No gain beyond noise and a lower parked_topic mean; the
  current prompt stays.
- **Ledger read excerpts: not measurable with these fixtures.** Fixture read outputs are raw
  text, not native read envelopes, so the ledger holds no reads in any eval case; the cache stays.
- **Maintenance: not measurable with these harnesses** (they force checkpoints directly; probe
  turns stay far below 0.83). It stays (see "Not changed").

## Before/after (benchmark audit)

Same grader, 3 trials, `main` = c34da50 + harness commit, branch = e04bf5e runners.

Compaction suite (no tools; probes answered from the post-compaction context):

| Metric (mean +- SD) | main | branch (full run) | branch (parked cases rerun) |
| --- | --- | --- | --- |
| Summary-only retention | 0.927 +- 0.054 | 0.921 +- 0.046 | 0.979 +- 0.036 |
| parked_topic (target 0.90) | 0.976 +- 0.041 | 0.833 +- 0.083 | 0.976 +- 0.041 |
| decision (target 0.85) | 0.556 +- 0.193 | 0.889 +- 0.193 | 1.000 +- 0.000 |
| middle position (target 0.85) | 0.978 +- 0.038 | 0.864 +- 0.067 | n/a (2 cases) |
| tool_result | 0.857 +- 0.143 | 0.905 +- 0.083 | n/a |
| Probe request errors per trial (excluded) | 0.0 | 0.7 (HTTP 429) | 0.0 |
| Requests per checkpoint (retries included) | 2.04 | 1.02 | 1.13 |
| Checkpoint latency | 33.1 s | 17.5 s | 19.9 s |
| Wall time (3 trials) | 20.7 min | 17.6 min | 4.8 min |

The full-run parked_topic and middle drops come from one case: `parked-budget-recipe` middle
probes were 5/9 in the branch's full run, 9/9 in its rerun, and also 5/9 in the staged
lean-prompt arm, so the swing is not specific to single pass (pooled single-pass runs 30/36,
pooled staged runs 14/18 on that case).

Behavior suite (`python3 -m evals.compaction behavior --trials 3`, same grader, recall offered):

| Metric | main | branch |
| --- | --- | --- |
| Greeting turns passing (short, no tools, no task mention) | 2/12 (0.17) | **7/12 (0.58)** |
| Unwanted resume (greeting + unrelated turns) | 0.48 | **0.29** |
| Greeting tool calls attempted | 5 | 4 |
| Greeting compactions | 0 | 0 |
| Unrelated question answered, task untouched | 1.00 | 0.89 |
| Topic return fact retention | 1.00 | 1.00 |
| Vague "continue" resumes the latest task | 0.89 | 1.00 |
| Wall time | 10.4 min | 7.7 min |

Remaining greeting failures are mostly "hey, how's it going?" (a status question that invites a
recap) and one 你好 turn that recapped the task.

Long-form recall (`python3 -m evals.longform run --trials 3`, 6 cases, agent with tools and
`recall`):

| Metric (mean +- SD) | main | branch |
| --- | --- | --- |
| Correct with valid citation, narrative | 33.3% +- 6.4 | 43.6% +- 7.9 |
| Correct with valid citation, code | 37.4% +- 14.8 | 32.9% +- 22.3 |
| Correct with valid citation, all | 34.8% +- 9.2 | 38.1% +- 6.6 |
| Correct, no citation requirement | 46.8% +- 7.1 | 48.7% +- 4.7 |
| Decoy hallucination | 20.0% +- 8.7 | 12.8% +- 6.3 |
| recall used | 56.3% | 64.3% |
| Original-only probes per trial | 31.3 | 28.3 |
| Input tokens per probe | 88,080 +- 1,340 | 93,416 +- 4,556 |
| Latency per probe | 67.2 s +- 10.1 | 68.1 s +- 11.8 |
| Invalid runs (checkpoint provider format error) | 1 (api-spec) | 2 (api-spec) |
| Wall time (3 trials, 4 jobs) | 93.9 min | 88.3 min |

All long-form differences are within noise. Input tokens per probe rose about 6% with more
recall calls (6.8 vs 6.3 tool calls per probe); the per-request system text is about 640 bytes
shorter.

## Live end-to-end run

```sh
cargo test -p themis-desktop --test compaction_cli_e2e --no-run
HOME=<fresh scratch>/home THEMIS_LIVE_COMPACTION=1 THEMIS_LIVE_DIR=<fresh scratch> \
  ./target/debug/deps/compaction_cli_e2e-<hash> live_ --ignored --nocapture
```

`OPENCODE_KEY` came from the environment into an in-memory store (the keychain is not read);
the server ran on the isolated data directory and every message went through the built `themis`
CLI (`create_thread`, `send_message`). Budget 24,000, `recent_messages` 4,
`longcat-2.5-preview-free`.

| Step | Outcome |
| --- | --- |
| Opening fact + 10 draft parts (release notes, then garden plan) | all finished; 2 checkpoints installed (after parts 5 and 9) |
| "In my release notes draft, what was the name of the beta build server? ... cite ..." | 3 tool calls (recall, then a read; an unattended shell call was denied); answer `KESTREL-77`, cited `snapshot-...jsonl` record 5, which is an `original` User record containing the fact (validated by the test) |
| "hi" | `Hi! How can I help you today?`; no checkpoint, no tool call, no mention of parked work |

A first live run asked for a fact the user had asked to remember; the checkpoint kept it, so the
model answered from the checkpoint without recall and cited a navigation record. The probe was
changed to a buried detail; that first run is not counted as recall evidence.

## Automated checks

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo clippy -p themis-core --features eval --all-targets -- -D warnings` | pass |
| `cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten` | 710 passed, 0 failed, 4 ignored |
| `cargo test -p themis-core --features eval` | pass |
| `npm --prefix web run lint` / `test` / `run build` | pass / 412 passed / pass |
| `ruff check quality_dashboard evals` | pass |
| `python3 -m unittest quality_dashboard.test_dashboard` | 14 passed |
| `python3 -m unittest evals.compaction.test_harness evals.longform.test_harness` | 48 passed |
| `cargo +nightly fuzz run --sanitizer none recall_search -- -max_total_time=60` | 170,190 runs, no crash |

New focused tests: `crates/desktop/tests/compaction_cli_e2e.rs` (shared server + CLI: a long chat
compacts and keeps working, recall cites the planted original record after compaction, "hi" after
a topic switch neither compacts nor gets anchors and the wire system text carries the greeting
rule, a failed checkpoint leaves the working context unchanged and the next run's checkpoint
still sees the early original, no `pending-job-` file); `oversized_history_compacts_in_one_request_and_leaves_the_rest_to_originals`
and `excerpted_checkpoint_keeps_the_middle_in_originals_after_a_failed_attempt` (runtime);
recall `rare_terms_outrank_common_ones_when_the_whole_question_is_the_query`, a determinism test
and a `term_weights` property test; provider-wire assertions on the system message in
`crates/core/tests/recall_e2e.rs` and the desktop recall/attachments tests.

## Not changed (and why)

- **Maintenance stays.** AGENTS.md carries maintenance-specific rules (protect delivered
  observations, keep recovery classification, check net savings, mixed-batch regressions);
  removing the layer would leave those rules describing nothing, which is a rule change for the
  lead to raise with the user. It is also the only layer that shrinks bulky recall/recovery
  results before a checkpoint, and no harness here exercises it.
- **Ledger read cache stays** (up to 64 KB of read excerpts per checkpoint at 200k). It overlaps
  with recall, but the fixtures cannot measure it; a fixture with native read envelopes is needed.
- **Staged pipeline and legacy stage-key migration stay for documents** (AGENTS.md document
  deadline and cache-identity rules).
- **Suspended progress stays**: it is the only record of completed writes after a failed
  checkpoint, which prevents replaying them.
- **No configuration field was removed or made legacy.** `compaction.max_stages` now applies to
  document preparation only (documented); old files still validate unchanged.
- Background/async summarization (roadmap item 6) is not started; a single request per
  checkpoint with a saved-snapshot input is the natural unit for it.

## Code health (quality dashboard)

`rust-code-analysis-cli` 0.0.25 through `quality_dashboard.dashboard.complexity_metrics()`; LOC
includes inline `#[cfg(test)]` code. Product overall: 141 files, mean cyclomatic 2.60, cognitive
2.13, maintainability 72.9 before and after.

| File | LOC | Functions | Cyclomatic | Cognitive | Maintainability |
| --- | --- | --- | --- | --- | --- |
| runtime.rs | 6,602 -> 6,644 | 156 -> 156 | 2.64 -> 2.62 | 2.34 -> 2.34 | 67.0 -> 67.1 |
| runtime/pending.rs | 138 -> 50 | 8 -> 3 | 2.23 -> 3.00 | 0.25 -> 0.50 | 66.8 -> 58.8 |
| runtime/summary_job.rs | 1,121 -> 1,101 | 26 -> 25 | 2.93 -> 2.98 | 2.00 -> 2.05 | 62.1 -> 62.2 |
| runtime/eval.rs | 816 -> 836 | 20 -> 20 | 2.49 -> 2.54 | 1.33 -> 1.37 | 72.7 -> 72.9 |
| tools/recall.rs | 1,366 -> 1,419 | 41 -> 42 | 2.91 -> 2.79 | 2.48 -> 2.31 | 73.0 -> 73.0 |

`crates/core/src` diff: +312 / -248 lines, most of the additions are tests. pending.rs's lower
maintainability index reflects removing its simplest functions (the remaining publish helper
is unchanged), not added complexity. Line coverage was not regenerated locally (CI runs
`cargo llvm-cov`).

## Limitations and unverified

- One free model, one day, 3 trials, synthetic fixtures, lexical grading. Run-to-run swings are
  large: the final branch's parked_topic measured 0.833 in the full run and 0.976 in a rerun of
  the two parked cases (single pass alone: 0.976, then 0.881).
- Topic-switch targets (parked >= 0.90, decision >= 0.85, middle >= 0.85) are not shown met with
  confidence: see the before/after table.
- Greeting behavior improved but greetings that ask "how's it going?" still get status replies,
  and some greeting turns still attempt tools.
- Desktop UI was not exercised (no UI code changed); the composer's context meter still reads the
  same `context_budget` event.
