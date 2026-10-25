# Budget eval

How the context budget changes **success, cost and time** on one task whose working set is small,
medium or large, so the budget Auto uses can be chosen from evidence. Design:
[docs/superpowers/specs/2026-10-24-budget-eval-design.md](../../docs/superpowers/specs/2026-10-24-budget-eval-design.md).

Results of live rounds are **benchmark audits**, not automated runtime checks. Only the harness
self-tests below run offline and automatically.

## The task

A generated project for a fictional ledger settlement service (`generate.py`), deterministic per
(size, seed):

- `ledger/desks/<desk>.py`: one skeleton module per trading desk (helpers, and `settle` not yet
  written), listed in `ledger.desks.DESKS`;
- `docs/overview.md` (the shared rules) and `docs/desks/<desk>.md`: each desk's own rules, drawn
  from the seed and written only on its page, in prose spread over its sections among operations
  notes, counterparties, an incident log and a history of retired rates: the desk code,
  currency, fee rate in basis points, rounding (half up, half even, down, up), minimum fee,
  rejection floor, two error numbers, line order, hold days, calendar and settlement tier;
- `data/<desk>.csv`: sample entries.

| Size | Desks | Material (bytes ÷ 4) |
| --- | --- | --- |
| small | 6 | ~30k tokens |
| medium | 30 | ~150k tokens |
| large | 90 | ~450k tokens |

The self-tests assert each size within ±20% of its target.

Three messages in one chat (`prompts/stage1.md` … `stage3.md`, filled with `{{snake_case}}`
placeholders like the runtime's prompt files, no trailing newline):

1. Implement `settle(entries)` in every desk per its page.
2. Change one rule from message 1 (the minimum fee of every desk in one tier) and add
   `holds(entries, today)` to every desk (it must still apply the message-1 rejection and
   order rules).
3. A command-line report, `python3 -m ledger report DATA_DIR --today DATE`, built from every desk.

The agent must write its own tests (`python3 -m unittest discover -s tests`), and they must pass.

## Grading

Hidden tests (generated per seed with the expected values computed by `reference/engine.py`)
run against a copy of the project in a sandbox, through `evals.hard.sheet.hidden` (on macOS no
network and no reads or writes under /Users or /Volumes); the agent's own tests run the same way.
They report per stage and per desk:

- stage 1: `settle` of every desk, on entries where the changed minimum cannot matter;
- stage 2: the changed minimum (and the unchanged one in other tiers) and `holds`;
- stage 3: the report, checked against the project's own `settle` and `holds`, so a wrong desk
  rule fails its own stage and not the report.

A run passes when all three stages and the agent's own tests pass and the chat finished.

The reference solution (`reference/`, written by `write_reference`) is never shipped to the agent.

## Running a round

```sh
python3 -m evals.budget run --sizes small,medium,large \
    --budgets 64000,128000,200000,400000,1000000 --trials 3 \
    --model MODEL --via runner --jobs 2 --out evals/budget/results/ROUND.json
```

- `--via runner` uses the tool-eval runner (`cargo build -p themis-core --features eval
  --example tool_eval`, debug, built on demand; `--runner` takes a prebuilt one). `--via app`
  (the default) uses the real `themis` app server and CLI (`--themis` for a prebuilt one), with a
  scratch data folder and HOME, YOLO approvals and the budget in the data folder's `runtime.jsonc`.
  The app accepts budgets only up to the maximum in its configuration schema (256000 today; the
  harness reads it from `crates/core/src/configuration.rs`), so **budgets above 256000 need
  `--via runner`** until the app's limit is raised.
- The key comes from `OPENCODE_KEY`; nothing else from your environment is used, and each run has
  its own scratch HOME.
- Arms are interleaved: trial by trial, each trial's (size, budget) pairs in a shuffled order. A
  trial uses one seed (`--seed` + trial − 1) for every budget and size, so budgets are compared
  on the same projects. Each budget is clamped to the model's window from models.dev; budgets
  that clamp to the same value run once.
- Before the round a preflight request lists the provider's models and stops if the model is
  missing. When a run fails with HTTP 404 or 410 (or "model not found"), the list is checked again
  and the round is aborted if the model is gone; arms not run are listed in the result.
- Defaults per size: small 2 h / 150 model turns per message, medium 4 h / 400, large 8 h / 1000
  (`--timeout`, `--max-turns`).
- `--dry-run` prints the prices, tiers and run order without running anything.
- The result file is written after every run, so stopping a round keeps what was graded.
  `--keep DIR` saves each run's folders.

### Spend control

Prices are models.dev's `cost` for provider `opencode-go`, in $ per million tokens (`input`,
`output`, `cache_read`, `cache_write`, and context tiers from `tiers` / `context_over_200k`).
Every cost is an estimate (≈) from the usage the provider reports.

- `--max-price` (default 0.20): the round is refused if the model's input price at any requested
  budget (its context tier included) is above it.
- `--max-round-cost` (default $5): a run starts only while the spend so far, plus the running
  runs (each at least its estimate) and the new run's estimate, stays within it. The estimate is
  the dearest finished run of that size, or before any, a deliberately high prior (20 input
  tokens per token of material), never above `--max-run-cost`.
- `--max-run-cost` (default $1.50): a run that passes it is stopped (the app's chat is stopped
  with `thread stop`; the runner is killed), graded as it stands and marked `stopped: cost`.
- The cost of a running run is computed live: the runner prints a content-free `eval-usage`
  counts line on stderr at each context-usage update, and the app streams `context_usage`
  events. Both report requests already completed, so a run can pass its limit by up to one
  request.
- A run whose budget is above a context tier is priced at that tier throughout (usage is totalled
  per run), and uncached input is priced at `cache_write` when that is dearer: both are upper
  bounds. Requests without reported input usage are covered by the byte estimate.
- Free models (all prices 0) are never held back or stopped for cost.
- The running spend is printed after every run and stored in the result (`spend`).

## Measures

Per run (`rows`): pass, stages passed, desks passed, own tests, input tokens and cached share,
output and reasoning tokens (reasoning is part of output), ≈ cost, wall time, requests,
compactions, tool calls, re-reads after a checkpoint and their share of the reads after the first
checkpoint (`evals.hard.moves`: the first read of a file after a checkpoint that the run had read
or written before it), and the failure kind: `stopped: cost`, `timeout`, `provider error`,
`cut-off` (a reply truncated at the output limit), `turn limit`, or `wrong answer`. Runs lost to
the provider before any tool call are listed under `errors` and excluded. A runner killed for
cost or time leaves no tool-call record, so its call measures are n/a.

```sh
python3 -m evals.budget report evals/budget/results/ROUND.json
python3 -m evals.budget compare before=A.json after=B.json
```

`report` prints mean ± sd per size × budget and the decision rule from the design: for the
large size, the smallest budget whose pass rate is within one standard error of the best; a
larger budget in that range wins only when it ties on pass rate and is both cheaper and faster.
It also says whether small and medium pass at every budget of 128k and above.

## Self-tests

```sh
python3 -m unittest evals.budget.test_harness
```

Offline: the reference passes every stage at every size; planted defects (one wrong rule, a
stage-1 rule forgotten by stage 2, the old minimum kept, no report) fail exactly their stage; the
generator is deterministic and in band; spend arithmetic, tiers, the round and run limits; the
schedule; the decision rule. `RunnerEndToEndTests` builds the debug runner and runs one small arm
against a scripted OpenAI-compatible server (no key is passed), checking the result file, the
usage and the spend, and the cost stop. `AppPathTests` drives the app path against a stand-in
CLI (plumbing only; the real app path is exercised by live rounds).
