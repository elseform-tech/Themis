# Long-horizon eval

Measures whether the agent keeps its goal, early details, progress and notes across several
context checkpoints and chat turns, and finishes the job correctly. **Live-model benchmark audit; not an automated runtime check.**
The offline harness tests (`python3 -m unittest evals.horizon.test_harness`) are the
automated part.

## What it does

Jobs far bigger than the 12k-token context budget, so they cross several checkpoints:

| Job | What the agent does | What has to survive compaction |
| --- | --- | --- |
| `stories` | Summarise 24 stories (~5 KB each) into `report.txt`, one line each, in order | Progress (no missing, duplicate or re-read stories), the main character of story 1, the count |
| `modules` | Write `cards/mod-NN.md` for 16 modules (function names plus a one-sentence summary of a ~4.5 KB docstring), then `cards/SUMMARY.md`; scripts are not allowed | A rule given only in the prompt (first line `owner: heron` in every card file), `src/` left alone, the total |
| `modules-turns` | The `modules` job over four turns: modules 1–8 and stop; a progress question (no work); finish 9–16 and SUMMARY.md; then "what was the rule and the total?" | Progress across turns with no tool history, the rule from turn 1, the total |
| `ports` | Find every port shared by more than one of 14 service files (~4.5 KB each) and write `conflicts.md`; nothing can be written until every file is read | The port of every file until the end, the number of shared ports |
| `modules-free` | The `modules` job with the agent untied: scripts allowed and YOLO-style approvals (the OS sandbox still applies) | Measures what the memory-test restrictions cost |

Arms (not every job runs every arm):

- `no-notes`: the `notes` tool and its instruction are withheld.
- `notes-offered`: as in the app (tool plus instruction).
- `notes-asked`: the prompt also asks for notes, so persistence is measured even when the
  model would not keep notes by itself.
- `notes-advice` (`modules`, `modules-turns`): notes offered, and the prompt carries the
  read-one-write-one advice removed from the system prompt, to measure whether it helps.

Follow-up turns get the history the desktop sends: the latest checkpoint plus the text turns
(no tool calls), and the same notes file.

The runner is the tool-eval runner (`crates/core/examples/tool_eval.rs`) with the same
isolation: fresh scratch project, scratch HOME, `sandbox-exec` on macOS. Destructive actions
are refused (as by a cautious person in Custom mode) unless a job allows all. A run lost to
the provider (rate limit, cut-off response, timeout) is rerun once after two minutes
(`--no-retry` turns that off); runs still lost are reported as infrastructure errors and
excluded from the rates.

## Metrics

- **Pass**: deterministic end-state check of the files.
- **Notes kept at checkpoints**: at every checkpoint the runner reads the saved-notes block it
  carries and compares it with the latest notes the agent saved before it: `kept`, `stale`
  (older notes), `dropped` (none although notes were saved), `changed`, or `no_notes_yet`
  (not counted). Reported as the kept share of checkpoints that had saved notes.
- **Open at end**: runs whose notes file still has `pending`/`in_progress` lines at the end.
- **Progress reported** (`modules-turns`): the progress answer names 8 done and the right
  function count so far.
- **Early detail / count recalled**: the final answer names something from the start of the
  task (story 1's hero; the notes rule) and the right count.
- **Re-reads**: `read_file` calls on a source file already read (work redone after forgetting).
- **Loop warnings**: warnings for three unchanged results in a row (repeats never stop a run).
  **Old guard would stop (passed)**: runs with two warnings in one turn, which the former
  guard stopped, and how many of those passed anyway.
- **Failure causes**: one per failed run: `turn_limit`, `guard_stop` (older builds),
  `run_error`, `unfinished`, `wrong_content` (a card or report line), `wrong_total` (a right
  list with a wrong total), `wrong_result` (other output), `wrong_progress`,
  `wrong_final_answer`.
- Rule kept in files (`modules`), compactions, calls and input tokens per run.

## Run

```sh
export OPENCODE_KEY=...   # never commit or print it
python3 -m evals.horizon run --trials 3 --model longcat-2.5-preview-free
python3 -m evals.horizon run --task stories            # one job, all its arms
python3 -m evals.horizon report evals/horizon/results/<file>.json
```

Options: `--task NAME` (task or job, repeatable), `--jobs N` (default 2), `--max-turns N`,
`--timeout S` (default 2400), `--runner PATH`, `--sandbox auto|off`, `--out FILE`,
`--base-url URL`, `--no-retry`.

Limits: free models with few trials (wide error bars), synthetic jobs, lexical checks. For
real coding work across messages see `evals/project`.
