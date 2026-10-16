# Long-horizon eval

Measures whether the agent keeps its goal, early details and plan across several context
checkpoints in one long job. **Live-model benchmark audit; not an automated runtime check.**
The offline harness tests (`python3 -m unittest evals.horizon.test_harness`) are the
automated part.

## What it does

Two jobs, each far bigger than the 12k-token context budget, so they cross several checkpoints:

| Job | What the agent does | What has to survive compaction |
| --- | --- | --- |
| `stories` | Summarise 24 stories (~5 KB each) into `report.txt`, one line each, in order | Progress (no missing, duplicate or re-read stories), the main character of story 1, the count |
| `modules` | Write `notes/mod-NN.md` for 16 modules (function names plus a one-sentence summary of a ~4.5 KB docstring), then `notes/SUMMARY.md` | A rule given only in the prompt (first line `owner: heron` in every notes file), `src/` left alone, the total |

Each job runs in three arms:

- `no-plan`: `update_plan` and its instruction are withheld.
- `plan-offered`: as in the app (tool plus one instruction sentence).
- `plan-asked`: the prompt also asks for a plan, so persistence is measured even when the
  model would not plan by itself.

The runner is the tool-eval runner (`crates/core/examples/tool_eval.rs`) with the same
isolation: fresh scratch project, scratch HOME, `sandbox-exec` on macOS.

## Metrics

- **Pass**: deterministic end-state check of the files.
- **Plan kept at checkpoints**: at every checkpoint the runner reads the plan line of the task
  ledger and compares it with the latest plan the agent saved before it: `kept`, `stale`
  (an older plan), `dropped` (none although one was saved), `changed`, or `no_plan_yet`
  (not counted). Reported as the kept share of checkpoints that had a saved plan.
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

Limits: one free model, synthetic jobs, lexical checks, a single message per job (no
multi-turn conversation yet).
