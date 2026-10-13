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
  (mean, sample SD, min, max across trials), 8 synthetic cases with 87 probes.

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
| `cargo test -p themis-core --features eval --test compaction_eval` | 2 passed (every committed case compacts and is probed through the runtime against wiremock; malformed transcripts rejected) |
| `python3 -m unittest evals.compaction.test_harness` | 9 passed (grader, aggregation, fixture shape, harness driving the real example against a local scripted HTTP server) |
| `ruff check evals` | pass |

CI runs the three eval-related commands above on the Linux/macOS/Windows matrix (see the PR).

## Live baseline (benchmark audit, not an automated check)

Setup: model `longcat-2.5-preview-free` (OpenCode Go, non-thinking), key from
`OPENCODE_KEY`, 3 trials x 8 cases, `--recent 20` (the app default), 3 parallel runs,
git commit recorded in each JSON. Commands:

```sh
python3 -m evals.compaction run --suite core --trials 3 --model longcat-2.5-preview-free --budget 24000
python3 -m evals.compaction run --suite core --trials 3 --model longcat-2.5-preview-free --budget 200000
```

Results: `evals/compaction/results/2026-10-13-longcat-2.5-preview-free-24000.json` and
`...-200000.json` (include every probe answer, verdict and provider error). No run crashed
or timed out (`failed_runs` empty); 18 forced compactions per trial, all installed.

Mean +- sample SD across 3 trials (pooled over the 8 cases; 79 scored probes + 8 decoys per trial):

| Metric | 24k budget | 200k budget |
| --- | --- | --- |
| Retention (correct / scored probes) | 0.890 +- 0.026 | 0.924 +- 0.051 |
| Forgot (answered `UNKNOWN`, fact existed) | 0.072 +- 0.007 | 0.046 +- 0.041 |
| Stale-fact rate (11 probes with stale patterns) | 0.000 +- 0.000 | 0.000 +- 0.000 |
| Wrong answers per trial | 0.67 +- 0.58 | 0.33 +- 0.58 |
| Invented decoy answers per trial (of 8) | 0.0 +- 0.0 | 0.33 +- 0.58 |
| Probe request errors per trial (excluded) | 2.7 +- 1.5 | 2.0 +- 1.0 |
| Retention, early / middle / late | 0.909 / 0.759 / 0.952 | 0.939 / 0.796 / 0.988 |
| Compaction latency (mean per compaction) | 30.2 s +- 7.3 | 17.4 s +- 3.0 |
| Estimated tokens before -> after | 17,670 -> 9,030 | 18,660 -> 11,000 |
| Provider requests per compaction | 1.89 | 1.04 |

Retention by category (24k / 200k): constraint 1.00 / 0.97, negation 1.00 / 0.92,
superseded 0.93 / 1.00, decision 0.93 / 0.93, narrative 0.96 / 1.00, next_action 0.96 / 1.00,
identifier 0.88 / 0.92, tool_result 0.89 / 0.92, parked_topic 0.69 / 0.78.

Retention by case, mean of 3 trials (24k / 200k): coding-csv-migration 1.00 / 0.96,
coding-rate-limiter 0.87 / 0.93, coding-rust-parser 0.89 / 0.89, constraints-api-design
0.97 / 1.00, narrative-lighthouse 0.94 / 1.00, parked-budget-recipe 0.67 / 0.76,
parked-trip-bugfix 0.85 / 0.89, superseded-deploy-plan 0.96 / 0.96.

### Observations

- Superseded values were never reported as current (0 stale answers in 33 stale-checked
  answers per budget); constraints and negations were retained nearly perfectly.
- The weak spot is detail inside *parked side topics* (recipe quantities, a gift date and
  budget) and mid-transcript facts: middle-third retention is 0.76-0.80 versus 0.91-0.99
  at the ends. The recurring misses are `UNKNOWN` answers about parked-topic details.
- At 24k the later compactions need ~3 provider requests (staged summaries; ~45 s) while the
  first ones need 1 (~15 s). At 200k almost everything fits one request.
- Tokens after compaction stay near 9k (24k) because the verbatim recent tail may use a
  third of the budget. At 200k the same transcripts keep more verbatim tail, so late-fact
  retention there partly measures the tail, not the summary.

### Failures and limitations

- **Probe errors.** 8 (24k) and 6 (200k) of 261 probe requests failed: 6 and 5 were
  "Provider response was truncated" (the model sometimes emits a pseudo tool call
  `<longcat_tool_call>` instead of answering within the 400-token limit) and 2 and 1 were
  upstream HTTP 429s. They are excluded from retention and counted in `probe_errors`; they
  slightly widen the uncertainty and mostly hit the same probes. The first 24k run (before
  error text was recorded) had a similar profile: retention 0.916, 6 errors.
- **Invented answer.** One 200k decoy probe (trial 2, narrative) was answered with a raw
  `<longcat_tool_call>` string instead of `UNKNOWN`; the grader counts it as invented
  although it is a format failure, not a fabricated fact. Treat the 0.33 as an upper bound.
- **Small sample.** 87 probes x 3 trials on one model; the SD is across only three trials,
  so differences below a few points are noise. One case (parked-budget-recipe) alone moves
  overall retention by about 3 points between trials.
- **Lexical grading.** A correct paraphrase that avoids every pattern is scored wrong; a
  hedged answer naming the new value passes. `wrong` answers should be read manually
  (details are in the JSON).
- **Synthetic fixtures.** Generated filler is more regular than real sessions; compaction
  was forced at fixed points rather than triggered by growth, and the summarizer is the same
  free model as the answerer (the app may route summaries differently).
- **200k budget.** Compaction was forced on 10-15k-token chunks, so this measures a
  single-stage checkpoint with a large verbatim tail, not a realistic 200k session.
- **Not covered:** thinking models, other providers, multi-session resumes, the maintenance
  (offload) tier, and UI behavior. No desktop/browser verification was needed or done.
