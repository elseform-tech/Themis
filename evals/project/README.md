# Real-project eval

A five-message coding job on a small Python project, graded automatically at every stage.
It checks what the long-horizon jobs cannot: real code changes that have to work, rules that
must hold for the whole chat, a bug found from its symptom, and honest progress reports.
**Live-model benchmark audit; not an automated runtime check.** The offline grader tests
(`python3 -m unittest evals.project.test_harness`) are the automated part.

```sh
python3 -m evals.project run --trials 3 --model longcat-2.5-preview-free --memory normal
python3 -m evals.project report evals/project/results/<file>.json
```

## The project

`template/` is an order-pricing package (catalog CSV, order files, bundle offers, tax per
category, a text report, JSON storage and a command line) with 21 tests and short docs, about
700 lines. A seed picks the package name, the catalog and sample orders in `data/`, and the
hidden test cases, so each trial is a project the model has not seen. One bug is planted: two
lines of the same product do not count together towards its bundle.

## The five messages

| # | Message | Graded on the snapshot after it |
| --- | --- | --- |
| 1 | Discount rules (percent per category, fixed per unit, order threshold) with an exact spec and API; three rules for the whole job (integer cents only, never touch `tests/legacy/`, docstrings on new public code); do the parser and pricing with tests, then stop | Hidden tests for parsing and pricing pass; own tests pass; rules kept; CLI, report and docs untouched |
| 2 | Progress check, change nothing | The answer names the number of test methods actually added and the CLI as left |
| 3 | A customer was overcharged (the planted bug, given only as an order and two totals) | Hidden split-line tests pass, earlier hidden tests still pass, own tests pass, rules kept, CLI still untouched |
| 4 | Finish: `--discounts` on the CLI, a `Discounts:` report line, storage, a docs section; run the suite | All hidden tests (including CLI and storage), the docs section names the three rule kinds, own tests pass, rules kept |
| 5 | "What rules did I give, and what changed?" | The answer names cents, legacy, docstrings, discounts and the bundle fix |

A run passes when all five stages pass. Each failure gets one cause: the turn limit, or the
first failed stage with why (broke a rule, own tests fail, did not stop where told, docs, or
wrong or unfinished).

## Setups

- Memory: `normal` uses the app's defaults (200,000-token context, 200 turns); `squeezed`
  (32,000 tokens) forces many checkpoints.
- Arms: `notes` (the app as it is), `no-notes` (the notes tool withheld), one runtime nudge
  switched off at a time (`no-reminder`, `no-finish-checks`, `no-loop-warning`), and
  `bash-only` (the file tools hidden, so the agent works through the shell), and `reasoning`
  (a thinking model's reasoning sent back with its own tool calls; it is never shown or saved).
  A nudge that does not clearly help when switched off is a candidate for removal.
- `--keep DIR` saves each run's `.themis/` (every tool call with its full result) for reading.

## Isolation

Approvals are YOLO-style, as a developer would use them (every action runs; the OS sandbox
still applies), and scripts are allowed. A run lost to the provider is rerun once after two
minutes (`--no-retry` turns that off). The runner is the tool-eval runner with `snapshots` on: it copies the project (without
`.themis`, `.git` and caches; symlinks skipped) after each message. Runs use a scratch project
and HOME and `sandbox-exec` on macOS. The grader runs agent code only in scratch copies, in a
subprocess with a time limit and, on macOS, `sandbox-exec` with no network and no file access
under `/Users` or `/Volumes`. The untouched project is regenerated from the seed for grading.

## Limits

The project is small and the spec exact, so this measures following a spec across messages,
not design judgement. Answers are checked by keywords and numbers, not read. Free models and
few trials give wide error bars.
