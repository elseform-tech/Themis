# Compaction checkpoint evals: harness and first live baseline (2026-10-13)

Two kinds of evidence, kept apart:

- **Automated runtime checks (CI-safe, offline):** prove the eval plumbing works. They
  say nothing about summary quality.
- **Live-model benchmark audit (not in CI):** the baseline below, one free model, one
  day, three trials. It is a reference point, not a regression gate.

See `evals/compaction/README.md` for how to run the suite and add cases.

## What was built

- `themis-core` cargo feature `eval` (never in releases) exposes `runtime::eval`:
  `parse_transcript`, `compact_chunks` (forces one compaction per chunk through
  `compact_context_with_evidence` with saved originals, so repeated compactions carry the
  prior checkpoint forward) and `answer_probe` (asks one question with the post-compaction
  messages only, no tools). It calls the existing internals; production code paths are
  unchanged apart from the `pub mod eval` declaration.
- `crates/core/examples/compaction_eval.rs` (`required-features = ["eval"]`) runs one case
  and prints a JSON report (per-chunk tokens before/after, duration, installed, provider
  requests; probe answers).
- `evals/compaction/` stdlib-only Python harness (`python3 -m evals.compaction run ...`),
  deterministic grader (any-of / regex / `UNKNOWN` / stale patterns / decoys), aggregation
  (mean, sample SD, min, max across trials), 8 synthetic cases with 87 probes. Headline retention is
  **summary-only**: probes whose planted record is no longer verbatim in the post-compaction
  context (the eval reports `verbatim_records` computed from the actual working messages).
  Retention for facts still sitting in the verbatim tail is reported separately, and a trial
  whose compaction failed is marked invalid and excluded.

Cases: 3 coding sessions (`coding-rate-limiter`, `coding-csv-migration`,
`coding-rust-parser`), 2 parked-topic (`parked-trip-bugfix`, `parked-budget-recipe`),
2 superseded/negation/constraint (`superseded-deploy-plan`, `constraints-api-design`),
1 narrative (`narrative-lighthouse`). Each has 2-3 chunks (so 2-4 forced compactions per
run) of roughly 10-15k estimated tokens, facts planted in the first, middle and last thirds,
1 decoy probe (a fact never stated; the right answer is `UNKNOWN`) and one next-action probe.

## Automated checks

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo clippy -p themis-core --features eval --all-targets -- -D warnings` | pass |
| `cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten` | all `test result` lines ok |
| `cargo test -p themis-core --features eval --test compaction_eval` | 3 passed (every committed case compacts and is probed through the runtime against wiremock, first record summarized and last verbatim; a failing summarizer invalidates the trial; malformed transcripts rejected) |
| `python3 -m unittest evals.compaction.test_harness` | 14 passed (grader incl. known-wrong answers for each tightened probe, planted-record check, tail/invalid-run aggregation, fixture shape, harness driving the real example against a local scripted HTTP server) |
| `ruff check evals` | pass |

CI runs the three eval-related commands above on the Linux/macOS/Windows matrix (see the PR).

## Live baseline (benchmark audit, not an automated check)

Setup: model `longcat-2.5-preview-free` (OpenCode Go, non-thinking), key from
`OPENCODE_KEY`, 3 trials x 8 cases, `--recent 20` (the app default), 3 parallel runs, git
commit recorded in each JSON.

```sh
python3 -m evals.compaction run --suite core --trials 3 --model longcat-2.5-preview-free --budget 24000
```

### Corrected baseline (24k budget, current grader and fixtures)

File: `evals/compaction/results/2026-10-13-longcat-2.5-preview-free-24000-v2.json`.
No run crashed, no trial was invalid, 18 forced compactions per trial all installed.
Per trial there are 79 scored probes: **55 summary-only** (fact no longer verbatim) and
**24 tail** (planted record still verbatim after compaction), plus 8 decoys.

| Metric (mean +- sample SD over 3 trials) | Value |
| --- | --- |
| **Summary-only retention (headline)** | **0.897 +- 0.042** (range 0.873-0.945) |
| Tail retention (record still verbatim) | 0.986 +- 0.024 |
| Overall retention (secondary) | 0.924 +- 0.034 |
| Forgot (answered UNKNOWN) | 0.034 +- 0.026 |
| Stale-fact rate (11 probes with stale patterns) | 0.000 |
| Wrong answers per trial | 1.3 +- 0.6 |
| Invented decoy answers per trial (of 8) | 0.0 |
| Probe request errors per trial (excluded) | 2.0 +- 1.0 |
| Compaction latency / tokens before -> after | 25.0 s +- 1.4 / 17,660 -> 9,020 |
| Provider requests per compaction | 1.89 |

Summary-only retention by category (overall in parentheses): constraint 1.00 (1.00),
negation 1.00 (1.00), identifier 0.92 (0.92), narrative 0.95 (0.96), superseded 0.92 (0.96),
parked_topic 0.83 (0.84), tool_result 0.81 (0.89), decision 0.78 (0.87). The next_action
probes are all in the tail (0.96 overall).

By position of the planted fact, summary-only (overall): early 0.931 (0.931),
middle 0.800 (0.833), late 0.944 (0.975).

By case, overall retention, mean of 3 trials: coding-csv-migration 1.00, coding-rate-limiter
0.90, coding-rust-parser 0.89, constraints-api-design 1.00, narrative-lighthouse 0.94,
parked-budget-recipe 0.79, parked-trip-bugfix 0.96, superseded-deploy-plan 0.93.

Wrong answers (4 across 3 trials): two `bug_file` answers named `src/range/caret.rs` but
not `bump_index` (the grader requires both), one misspelled name ("Marit Oyren"), and one
`why_eu` answer that gave the later Frankfurt/latency reason instead of the EU data rule.
Probe errors: 5 "Provider response was truncated" (the free model sometimes emits a pseudo
tool call instead of answering) and 1 request timeout; all excluded from retention.

### Earlier (superseded) numbers and why they differ

The first PR revision reported headline retention of 0.890 (24k) and 0.924 (200k) as overall
retention over all probes, graded with looser patterns. A review found two problems:

1. 26 of 87 probes were planted in the last records, which stay verbatim, so overall
   retention overstated what the checkpoint preserves. The reviewer's split of the original
   24k run by an assumed 20-record tail was 0.855 (non-tail) vs 0.962 (tail).
2. Some expected patterns accepted wrong answers (`comma|,`, bare `EU`, `not` inside
   "note"/"nothing") and several `planted_at` values were one record off.

Both are fixed. The original files are kept unchanged, and were regraded offline with the new
grader and fixtures (`python3 -m evals.compaction regrade <file>`, same stored model answers):

| Run | As first reported (overall) | Regraded (overall) | Summary-only split |
| --- | --- | --- | --- |
| 24k, first run | 0.890 +- 0.026 | 0.878 +- 0.019 (wrong answers 0.67 -> 1.67 per trial) | not available |
| 200k, first run | 0.924 +- 0.051 | 0.924 +- 0.051 | not available |
| 24k, fresh run `-v2` | n/a | 0.924 +- 0.034 | **0.897 +- 0.042** |

The first-run files did not record which records stayed verbatim (`verbatim_records`), so the
summary-only/tail split cannot be reconstructed from them; the regraded copies
(`...-regraded.json`) say so. Fixture transcripts did not change, only `planted_at`, `needle`
and expected patterns in `probes.json`, so regrading is exact for the stored answers. The
fresh run is a different sample of model output: do not read the 0.878 -> 0.924 overall change
as an improvement, three trials of one free model have an SD of 2-5 points. The 200k first
run was not repeated; its numbers should be treated as overall (tail-inflated) retention.

### Failures and limitations

- **Probe errors.** 6 of 261 probe requests failed (5 truncation, 1 timeout) and are excluded;
  they hit different probes between runs.
- **Small sample.** 87 probes x 3 trials on one model; the summary-only SD (0.042) means
  differences of a few points are noise. 55 summary-only probes per trial; one case
  (parked-budget-recipe) alone moves overall retention by about 3 points.
- **Tail vs summary.** `verbatim` is decided by exact message equality with the original
  record; a record whose tool arguments were offloaded counts as summarized. A fact stated
  in several records counts as summary-only only if its planted record is gone.
- **Lexical grading.** A correct paraphrase that avoids every pattern is scored wrong (see
  the `bug_file` answers); abstention detection covers common phrasings only. Read `wrong`
  answers in the JSON.
- **Synthetic fixtures.** Generated filler is more regular than real sessions; compaction was
  forced at fixed points rather than triggered by growth, and the summarizer is the same free
  model as the answerer.
- **Not covered:** thinking models, other providers, multi-session resumes, the maintenance
  (offload) tier, and UI behavior. No desktop/browser verification was needed or done.
