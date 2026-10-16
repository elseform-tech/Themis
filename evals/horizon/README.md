# Long-horizon eval

Measures whether the agent keeps its goal, early details, progress and notes across several
context checkpoints and chat turns, and finishes the job correctly. **Live-model benchmark audit; not an automated runtime check.**
The offline harness tests (`python3 -m unittest evals.horizon.test_harness`) are the
automated part.

## What it does

Two jobs, each far bigger than the 12k-token context budget, so they cross several checkpoints:

| Job | What the agent does | What has to survive compaction |
| --- | --- | --- |
| `stories` | Summarise 24 stories (~5 KB each) into `report.txt`, one line each, in order | Progress (no missing, duplicate or re-read stories), the main character of story 1, the count |
| `modules` | Write `notes/mod-NN.md` for 16 modules (function names plus a one-sentence summary of a ~4.5 KB docstring), then `notes/SUMMARY.md` | A rule given only in the prompt (first line `owner: heron` in every notes file), `src/` left alone, the total |

| `modules-turns` | The `modules` job over four turns: modules 1–8 and stop; a progress question (how many done, which left, function count so far; no work); finish 9–16 and SUMMARY.md; then "what was the rule and the total?" | Progress across turns with no tool history, the rule from turn 1, the total |

Each job runs in three arms:

- `no-notes`: the `notes` tool and its instruction are withheld.
- `notes-offered`: as in the app (tool plus instruction).
- `notes-asked`: the prompt also asks for notes, so persistence is measured even when the
  model would not keep notes by itself.

Follow-up turns get the history the desktop sends: the latest checkpoint plus the text turns
(no tool calls), and the same notes file.

The runner is the tool-eval runner (`crates/core/examples/tool_eval.rs`) with the same
isolation: fresh scratch project, scratch HOME, `sandbox-exec` on macOS.

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
- Rule kept in files (`modules`), compactions, calls and input tokens per run.

## Run

```sh
export OPENCODE_KEY=...   # never commit or print it
python3 -m evals.horizon run --trials 3 --model longcat-2.5-preview-free
python3 -m evals.horizon run --task stories            # one job, all three arms
python3 -m evals.horizon report evals/horizon/results/<file>.json
```

Options: `--task NAME` (task or job, repeatable), `--jobs N` (default 2), `--max-turns N`,
`--timeout S` (default 2400), `--runner PATH`, `--sandbox auto|off`, `--out FILE`,
`--base-url URL`.

Limits: one free model, synthetic jobs, lexical checks.
