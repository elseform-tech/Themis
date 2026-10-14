# Long-form recall evals: harness and first live baseline (2026-10-14)

Two kinds of evidence, kept apart:

- **Automated runtime checks (CI-safe, offline):** prove the plumbing (generator, grader,
  citation validator, in-context exclusion, a scripted agent run). They say nothing about model
  quality.
- **Live-model benchmark audit (not in CI):** the baseline below, one free model, one day, three
  trials, current `main` (5e2a489) plus this harness. It is a reference point, not a regression
  gate.

See `evals/longform/README.md` for how to run it, add cases and compare two builds.

## What was built

- `runtime::eval::answer_probe_agent` (cargo feature `eval`, never in releases) starts a real agent
  turn from the post-compaction messages (the same durable checkpoint form the app resumes from),
  with the default toolset of `boxed_tools_for_chat` over a scratch project that holds the saved
  originals at `.themis/context/eval-thread/snapshot-*.jsonl`. Approval policy: read tools; shell
  limited to `grep rg cat head tail wc ls jq nl cut tr` (interpreters, `sed`, `awk`, `find` and
  writes are refused); at most 8 executed actions per probe. The scratch directory is a fresh
  system temp dir, deleted afterwards; the policy guards that directory and is not a sandbox.
  `examples/longform_eval.rs` runs one case end to end.
- Why in-process rather than the `themis` CLI: forced compactions at exact record boundaries and
  the post-compaction message list (needed for the in-context exclusion) are only available
  in-process, and the runtime path is the same one the app uses.
- `evals/longform/`: six deterministic cases from a committed seeded generator, deterministic
  anchored grader, citation validator against the snapshots, in-context exclusion, mean/SD
  reporting, `run`, `report` and `regrade` commands, `--runner` / `--build-ref` for comparing builds.

| Case | Kind | Material | Compactions | Probes (detail / quote / decoy) |
| --- | --- | --- | --- | --- |
| chronicle | narrative | 12 chapters, ~136 KB | 6 | 9 / 2 / 2 |
| letters | narrative | ~73 letters in 6 bundles, ~66 KB | 6 | 9 / 2 / 2 |
| meeting | narrative | 8 agenda items, ~111 KB | 4 | 9 / 2 / 2 |
| api-spec | code | 7 spec parts + amendments, ~68 KB | 8 | 11 / 1 / 2 |
| build-log | code | 8 test shards, ~66 KB | 8 | 9 / 1 / 2 |
| refactor | code | 5 sessions of tool calls, ~68 KB | 5 | 9 / 2 / 2 |

(probe counts as committed in `probes.json`; retraction and attribution probes are counted under
detail.) Every fact is a unique generated string; the generator fails if a probe's evidence appears
in any record other than the planted ones, and decoy keywords never occur in a transcript.

## Automated checks

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo clippy -p themis-core --features eval --all-targets -- -D warnings` | pass |
| `cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten` | all `test result` lines ok (254 core lib tests included) |
| `cargo test -p themis-core --features eval` | all ok, including the policy unit test and `compaction_eval` |
| `python3 -m unittest evals.longform.test_harness` | 19 passed: byte-for-byte generator determinism, case shape, probe/evidence uniqueness, grader known-wrong answers for every committed probe, citation parsing/validation, in-context exclusion, offline end-to-end runs of the real example against a scripted provider (perfect agent cited and scored; bad citations and ignorance do not pass), scratch dirs removed |
| `ruff check evals quality_dashboard` | pass |

## Live baseline (benchmark audit, not an automated check)

Setup: `longcat-2.5-preview-free` via OpenCode Go, 24k budget, `--recent 20`, 8 tool calls per
probe, 3 trials x 6 cases, 6 parallel runs. 37 forced compactions per trial, all installed; no
invalid or crashed runs.

```sh
python3 -m evals.longform run --trials 3 --model longcat-2.5-preview-free --budget 24000
```

Result file: `evals/longform/results/2026-10-14-longcat-2.5-preview-free-24000.json` (run with the
harness at ebf5314, then re-graded offline with `python3 -m evals.longform regrade` after the
decoy-abstention rule was widened; model output and citation checks are unchanged). Probes whose
answer is present in the post-compaction context are excluded from the headline: 37 per trial
(20 in the checkpoint summary, 17 in the verbatim tail), leaving 29 original-only probes per trial.

| Metric (mean ± sample SD over 3 trials) | Narrative | Code | All |
| --- | --- | --- | --- |
| Original-only probes per trial | 13.3 ± 2.5 | 15.7 ± 1.2 | 29.0 ± 3.6 |
| **Correct with valid citation (headline)** | **33.5% ± 14.5** | **25.1% ± 8.8** | **29.0% ± 11.7** |
| Correct, no citation requirement | 60.9% ± 21.7 | 48.2% ± 14.3 | 53.5% ± 12.7 |
| Citation valid, of correct answers | 57.8% ± 23.4 | 51.5% ± 2.6 | 53.7% ± 13.4 |
| Answered UNKNOWN | 11.9% ± 11.6 | 25.9% ± 7.9 | 19.7% ± 7.7 |

| Other metric | Value |
| --- | --- |
| Exact-quote accuracy (original-only) | 13.3% ± 23.1 |
| Decoy hallucination rate | 11.1% ± 9.6 |
| Accuracy on in-context probes (secondary, not headline) | 76.9% ± 4.7 |
| Mean tool calls per probe, attempted / executed | 9.2 ± 0.3 / 5.8 ± 0.1 |
| Probes with a refused call (almost all `python3`; a few `write_file`) | 73.1% ± 4.6 |
| `recall` available / used | no / 0% |
| Provider requests per probe | 8.6 ± 0.2 |
| Provider input / output tokens per probe | 118,881 ± 1,502 / 2,481 ± 129 |
| Latency per probe | 81 s ± 7 |
| Provider/transport errors (excluded) | 0 |

By case (original-only probes per trial; headline / no-citation requirement): api-spec (4)
0.69/0.69, letters (2) 0.50/0.50, chronicle (1) 0.00/0.67, meeting (10) 0.35/0.63, refactor (3)
0.30/0.48, build-log (8) 0.04/0.40. Several cases leave very few original-only probes because the
checkpoint or the verbatim tail already holds the fact, so per-case numbers are noisy; only the
pooled rows are worth reading. By category (accuracy without citation): attribution 0.58, detail 0.58,
retraction 0.67 (9 probes over 3 trials), quote 0.13.

Reading the numbers: of the 47 correct answers among the 87 original-only probe answers pooled over the three trials, 11
cited a record that does not state the fact and 10 cited nothing, so the citation requirement
removes about 45% of correct answers. The model also cannot reproduce exact quotes (13%), spends
about 9 attempted calls per probe, a third of them refused (it keeps trying `python3`, which the
system prompt suggests), and two probes ended with "Repeated unchanged tool results stopped the
run" (counted as wrong). Why build-log scores worst on the citation requirement was not
investigated (unverified; one hypothesis is that it cites later snapshots where the shard output
was offloaded to a receipt).

### Limitations

- **Small, noisy sample.** About 29 original-only probes per trial, one free model, three trials;
  SDs of 9-15 points mean differences smaller than that are noise. Which probes count as
  original-only varies with each trial's summary.
- **Selection effect.** The headline is restricted to details the checkpoint and tail did not
  retain, so a better summarizer shrinks the set rather than raising the score; compare the
  excluded counts (and in-context accuracy) alongside the headline when comparing builds.
- **Lexical grading.** A correct paraphrase that avoids the anchored pattern is wrong; a hedged
  answer that leads with the right value passes; answers over 800 characters count as wrong;
  abstention detection is phrase-based (widened after the first run, see above).
- **Citation strictness.** A citation is valid only when it names a `snapshot-*.jsonl` record that is
  an original (not navigation or recovery) record stating the fact; a correct reference to the
  source elsewhere (e.g. a tool-offload receipt) is not credited.
- **Policy interaction.** The agent is steered by the app's system prompt toward `python3`, which
  this eval refuses; those refusals cost calls. The tool set is `boxed_tools_for_chat`; `recall`
  will only appear if the build registers it there (`recall_available` and per-probe `tools` are
  recorded either way).
- **Synthetic material** is more regular than real sessions; compaction is forced at record
  boundaries, and the summarizer and answerer are the same free model.
- Not covered: thinking models, other providers, UI behavior (no desktop/browser verification was
  needed or done).
