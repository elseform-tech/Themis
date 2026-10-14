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
  with the default toolset of `boxed_tools_for_chat` over a scratch project that holds only the
  saved originals at `.themis/context/eval-thread/snapshot-*.jsonl` (plus `recall`, read-only and
  scoped to the same directory, when the build has it: the harness passes `--cfg eval_recall`
  if `tools/recall.rs` exists in the tree).
- **Confinement of the answer agent** (tightened after review):
  - Approval policy: read tools; shell limited to `grep rg cat head tail wc ls jq nl cut tr`;
    interpreters, `sed`, `awk`, `find`, writes and the `git` tool are refused (`git log --output=`
    writes anywhere, `git diff --no-index` reads anywhere); at most 8 executed actions per probe.
  - Every non-flag shell argument, and every path-like flag value (`--file=X`, `-fX`), must resolve
    (canonicalized, symlinks followed) inside the scratch project; `..`, `~` and outside absolute
    paths are refused and recorded as `refused_paths` in the results.
  - The runner is started with an isolated environment (scratch `HOME`/`TMPDIR`, no real app-data
    dir, only `PATH`, the provider key and TLS variables) and only the fixture inputs are copied to
    scratch (`probes.json` stripped to id and question; the answer key stays in the repository).
  - On macOS the runner is wrapped in `sandbox-exec` (reads and writes under `/Users` and
    `/Volumes` denied except the runner binary and the scratch directory). **Network is not
    restricted** (the provider host cannot be allow-listed by name), and the sandbox is only applied
    on macOS; elsewhere the argument confinement and isolated environment are the only guards.
  - The scratch directory is a fresh system temp dir, deleted afterwards. `file_path`-style tools
    (`read_file`, `list_dir`, `search_file`) are confined by their toolkit root (not re-audited here).
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
| `python3 -m unittest evals.longform.test_harness` | 27 passed: byte-for-byte generator determinism, case shape, probe/evidence uniqueness, grader known-wrong answers for every committed probe, citation parsing/validation and the 3-record cap, hedged and strict-abstention grading, error redaction, stripped inputs, isolated environment, macOS sandbox, refused absolute-path reads through the real runner, in-context exclusion, offline end-to-end runs of the real example against a scripted provider (perfect agent cited and scored; bad citations and ignorance do not pass), scratch dirs removed |
| `ruff check evals quality_dashboard` | pass |

## Live baseline (benchmark audit, not an automated check)

**The target (at least 80% original-only accuracy with valid citation) is not met on the main
baseline: 22.5% (narrative 21.6%, code 23.1%).** With 3 trials and an SD of about 7-12 points per
group, only differences of roughly 20 points or more between two builds are meaningful; use more
trials (5 if rate limits allow) for the recall comparison.

Setup: `longcat-2.5-preview-free` via OpenCode Go, 24k budget, `--recent 20`, 8 tool calls per
probe, 3 trials x 6 cases, 6 parallel runs. 37 forced compactions per trial, all installed; no
invalid or crashed runs.

```sh
python3 -m evals.longform run --trials 3 --model longcat-2.5-preview-free --budget 24000
```

Provenance. The model run happened with the first harness version (shell not sandboxed, `git`
tool allowed, raw error text stored). After review the answer agent was confined (see above) and
the grader was finalized (strict abstention rule for decoys, hedged answers counted wrong, at most
3 cited records). The stored model answers and citation checks were then **re-graded offline**
with that final grader (`python3 -m evals.longform regrade`), which is the grader the recall
comparison will use; the earlier, more lenient numbers (29.0% headline) are superseded. The run
was not repeated under the confined policy: the confinement can only remove capabilities (reads
outside the scratch project), and no refused out-of-project read was recorded in the first run
because that field did not exist yet, so treat the baseline as unverified under the new confinement.

Result file: `evals/longform/results/2026-10-14-longcat-2.5-preview-free-24000.json`. Probes whose
answer is present in the post-compaction context are excluded from the headline: 37 per trial
(20 in the checkpoint summary, 17 in the verbatim tail), leaving 29 original-only probes per trial.

| Metric (mean ± sample SD over 3 trials) | Narrative | Code | All |
| --- | --- | --- | --- |
| Original-only probes per trial | 13.3 ± 2.5 | 15.7 ± 1.2 | 29.0 ± 3.6 |
| **Correct with valid citation (headline)** | **21.6% ± 8.5** | **23.1% ± 5.4** | **22.5% ± 6.8** |
| Correct, no citation requirement | 39.3% ± 21.4 | 38.2% ± 4.2 | 38.3% ± 10.8 |
| Citation valid, of correct answers | 65.1% ± 35.7 | 60.5% ± 10.7 | 61.4% ± 22.4 |
| Hedged (two planted values; counted wrong) | 19.0% ± 11.8 | 10.1% ± 12.1 | 14.0% ± 11.5 |
| Answered UNKNOWN | 14.4% ± 15.5 | 23.7% ± 5.2 | 19.7% ± 7.7 |

| Other metric | Value |
| --- | --- |
| Exact-quote accuracy (original-only) | 6.7% ± 11.6 |
| Decoy hallucination rate (strict abstention) | 25.0% ± 8.3 |
| Citations over the cap of 3 records | 1.3% ± 2.2 |
| Accuracy on in-context probes (secondary, not headline) | 56.1% ± 4.1 |
| Mean tool calls per probe, attempted / executed | 9.2 ± 0.3 / 5.8 ± 0.1 |
| Probes with a refused call (almost all `python3`; a few `write_file`) | 73.1% ± 4.6 |
| `recall` available / used | no / 0% |
| Provider requests per probe | 8.6 ± 0.2 |
| Provider input / output tokens per probe | 118,881 ± 1,502 / 2,481 ± 129 |
| Latency per probe | 81 s ± 7 |
| Provider/transport errors (excluded) | 0 |

By case (original-only probes per trial; headline / no-citation requirement): api-spec (4)
0.61/0.61, letters (2) 0.50/0.50, chronicle (1) 0.00/0.67, meeting (10) 0.20/0.34, refactor (3)
0.30/0.41, build-log (8) 0.04/0.28. Several cases leave very few original-only probes because the
checkpoint or the verbatim tail already holds the fact, so per-case numbers are noisy; only the
pooled rows are worth reading.

Reading the numbers: over the 87 original-only probe answers pooled over the three trials, 33 were
correct, 23 wrong, 17 UNKNOWN, 13 hedged and 1 stale. The model cannot reproduce exact quotes, often
names several candidate values, and spends about 9 attempted calls per probe, a third of them
refused (it keeps trying `python3`, which the system prompt suggests). Two probes ended with
"Repeated unchanged tool results stopped the run" (counted as wrong). Why build-log scores worst on
the citation requirement was not investigated (unverified; one hypothesis is that it cites later
snapshots where the shard output was offloaded to a receipt).

### Limitations

- **Small, noisy sample.** About 29 original-only probes per trial, one free model, three trials;
  SDs of 7-12 points mean differences smaller than about 20 points are not meaningful. Which
  probes count as original-only varies with each trial's summary.
- **Selection effect.** The headline is restricted to details the checkpoint and tail did not
  retain, so a better summarizer shrinks the set rather than raising the score; compare the
  excluded counts (and in-context accuracy) alongside the headline when comparing builds.
- **Lexical grading.** A correct paraphrase that avoids the anchored pattern is wrong; answers over
  800 characters count as wrong; naming a second planted value (even as context) counts as hedged;
  a value the answer states before saying "not mentioned" still counts as invented for decoys.
- **Citation strictness.** A citation is valid only when it names at most three `snapshot-*.jsonl`
  records, one of which is an original (not navigation or recovery) record stating the fact.
- **Not a sandbox off macOS; network unrestricted on macOS.** See the confinement list above. The
  first live run predates the confinement (the model was not observed attempting out-of-project
  reads, but the data to show that was not recorded).
- **Policy interaction.** The agent is steered by the app's system prompt toward `python3`, which
  this eval refuses; those refusals cost calls. `recall` appears only in builds that contain
  `tools/recall.rs` (the harness detects it); a build-only check of `--build-ref codex/recall-tool`
  produced a runner that contains the recall tool description. No recall run was made.
- **Synthetic material** is more regular than real sessions; compaction is forced at record
  boundaries, and the summarizer and answerer are the same free model.
- Not covered: thinking models, other providers, UI behavior (no desktop/browser verification was
  needed or done).
