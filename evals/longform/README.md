# Long-form recall evals (agent mode)

Measures whether the agent can answer detail questions whose answer exists **only in the original
material** after several compactions, for narrative and code material. This is a **live-model
benchmark audit**, separate from the automated runtime checks. It is never run in CI against a real
model; CI only proves the plumbing offline (`python3 -m unittest evals.longform.test_harness`).

Unlike `evals/compaction` (no tools, short answers from the post-compaction context), every probe is
answered by a real agent turn with the default toolset (`read_file`, `list_dir`, `search_file`,
`shell`, ... and `recall` automatically if the build provides it) against the case's saved
originals.

## How it works

1. `crates/core/examples/longform_eval.rs` (feature `eval`) replays a case transcript and forces a
   compaction after each chunk boundary through the runtime (`compact_context_with_evidence`),
   saving originals as `snapshot-*.jsonl` under `<scratch>/project/.themis/context/eval-thread`
   (the app layout). It writes the post-compaction working messages to `<scratch>/context.json`.
2. For each probe it starts an agent turn (`runtime::eval::answer_probe_agent`) from the stored
   checkpoint (summary plus recent messages, exactly the durable form the app resumes from) with
   the question. Policy: read tools run; shell only for `grep rg cat head tail wc ls jq nl cut tr`;
   no interpreters, `sed`, `awk`, `find`, `git` or writes; every path-like argument must resolve
   inside the scratch project (refusals are recorded in `refused_paths`); at most `--max-calls`
   (default 8) tool actions. The runner gets an isolated `HOME`/`TMPDIR`, only the fixture inputs
   (probes stripped to id and question) are copied to scratch, and on macOS it is wrapped in
   `sandbox-exec` (`--sandbox off` to disable; network is not restricted, non-macOS has no sandbox).
   The scratch project is a fresh temp dir outside the repository, deleted afterwards. If the build
   has the `recall` tool (`crates/core/src/tools/recall.rs`), the harness compiles it into the
   eval toolset (`--cfg eval_recall`).
3. The Python harness (`python3 -m evals.longform run`) then, per probe:
   - **in-context exclusion**: if the probe's evidence regex matches one working message of the
     post-compaction context, the fact is not original-only; the probe is recorded as
     `in_context` (`checkpoint` or `tail`) and excluded from the headline (its accuracy is
     reported separately);
   - grades the answer deterministically (anchored regexes; quotes need the exact wording, case
     sensitive; superseded values listed first count as `stale`; an answer that also names another
     planted value is `hedged` and wrong; decoys need a real abstention: a leading `UNKNOWN`, or a
     first sentence that says nothing was found and names no candidate);
   - checks **grounding**: a correct answer counts only if the expected fact appears in what the
     agent's own tool calls returned during that probe (decoded JSON, at most 16 KiB per call).
     Probes are original-only, so the fact can only have come from the saved originals. Answers
     do not cite sources: the agent is told never to show archive paths or record numbers, and
     any answer that does (`snapshot-N.jsonl`, `.themis/context`, `record N`, `char_range`, ...)
     counts as a **source leak**. Stored errors are reduced to a class and HTTP status (no
     provider text).
4. Reports mean +- sample SD over trials. Trials whose compaction did not install are `invalid`
   and excluded.

## Run

```sh
export OPENCODE_KEY=...   # never commit or print it
python3 -m evals.longform run --trials 3 --model longcat-2.5-preview-free --budget 24000
python3 -m evals.longform report evals/longform/results/<file>.json   # markdown again
```

Options: `--case NAME` (repeatable), `--jobs N`, `--recent N`, `--max-calls N`, `--timeout S`,
`--base-url URL` (mock provider), `--sandbox auto|off`, `--out FILE`. Results go to `evals/longform/results/`.

## Compare two builds

- `--runner PATH`: a prebuilt `longform_eval` binary (build it in any checkout:
  `cargo build --release -p themis-core --features eval --example longform_eval`).
- `--build-ref REF`: check `REF` out in a temporary git worktree, overlay only the eval entry
  points (`runtime/eval.rs`, the example and its Cargo stanza) from this checkout, build, run and
  clean up. The tools, runtime and compaction under test are the ref's own, so
  `--build-ref codex/recall-tool` measures a branch that adds `recall`. The JSON records
  `tools_available`, `recall_available` and per-probe `tools`, so `used_recall` shows whether the
  agent used it. If `recall` is registered outside `boxed_tools_for_chat` it will not appear in
  this entry point; add it to the toolset in `answer_probe_agent` then.

Run both builds with the same `--model/--budget/--trials` and compare the headline rows.

## Grading and metrics

Headline: **original-only detail accuracy, grounded in what the agent read**, narrative vs code
(over probes not answerable from the post-compaction context). Also reported: accuracy without the
grounding requirement, grounded share of correct answers, source-leak rate, `unknown` and wrong
rates, exact-quote accuracy, decoy
hallucination rate, in-context counts and accuracy, mean tool calls, `recall` use, requests,
tokens, latency, and provider/transport errors (excluded, listed). Result files from before 2026-10-15 graded answer citations instead; their rows have no `grounded` field and the report falls back to `cite_valid`, so compare old and new headlines with care. Lexical grading limits: a correct
paraphrase that avoids the pattern is scored wrong, and a hedged answer that leads with the right
value passes even if it later mentions the superseded one.

## Cases

`cases/<name>/{transcript.jsonl,probes.json,case.json}`, generated by
`python3 -m evals.longform.gen_cases` (seeded, no copyrighted text; the unit test regenerates and
compares byte for byte).

| Case | Kind | Material |
| --- | --- | --- |
| chronicle | narrative | 12 chapters (~136 KB): cast, places, objects, dates, quotes |
| letters | narrative | ~73 letters (~66 KB): who-wrote-to-whom, quotes, sums and retractions |
| meeting | narrative | 8-item transcript (~111 KB): speakers, decisions later revised |
| api-spec | code | API design spec (~68 KB): field limits, scopes, error codes, amendments |
| build-log | code | 8 shards of test output (~66 KB): one failing test buried in shard 5 |
| refactor | code | 5-session tool-call history (~68 KB): bug introduced at one file:line, fixed at another |

Each has 4-8 forced compactions, 9-11 detail probes, 1-2 quote probes and 2 decoys. Probe fields:
`fact_records` (records stating the fact; the generator asserts the evidence regex matches exactly
these), `expected.regex`, optional `stale`, `exact` (quotes), `evidence` (regex of the original
text), `answer_text` (literal answer, used by the offline scripted provider only).

To add a case: write a builder with `Case(...)` in `gen_narrative.py`/`gen_code.py`, plant unique
tokens via `case.unique`, register it in `gen_cases.py`, regenerate and commit. Keep every answer a
unique string (the generator fails when the evidence appears in other records), at least three
chunk boundaries, and decoys whose keywords never appear.

## Recorded results

- `results/2026-10-14-longcat-2.5-preview-free-24000.json`: first main baseline (predates the confinement; regraded).
- `results/2026-10-14-compare-main-...json` and `...-compare-recall-...json`: main vs `codex/recall-tool`
  under the final confinement, run in parallel (see `docs/verification/longform-evals-2026-10-14.md`).
  Reproduce with `--runner` pointing at a runner built from each tree, or `--build-ref codex/recall-tool`.
- `results/2026-10-15-compare-main-c34da50-...json` and `...-compare-simplify-...json`: main vs
  `codex/simplify-compaction` (one summary request per checkpoint, recall-first system text,
  inverse-frequency recall ranking); `...-ablate-single-pass-meeting-build-log-...json` is the
  single-pass ablation on the two cases whose context checkpoints used to stage. See
  `docs/verification/simplify-compaction-2026-10-15.md`.
