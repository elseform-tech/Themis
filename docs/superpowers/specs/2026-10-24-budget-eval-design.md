# Budget eval: design

Status: approved 2026-10-24. Built as `evals/budget/` (see its README).

## Why

We want to choose the context budget that Auto uses (today: the model's window, at most 200k)
from evidence rather than guesswork. The current evals cannot answer this:

- `evals/hard` sheet-long fits in a small window: beam-free never compacted at 64k–1M, so the
  budget could not change the result.
- `evals/compaction` midturn's project is about 20k tokens and was built to stress 16–24k budgets.
- The retention and behavior evals are synthetic checks of summaries at deliberately small budgets.
- Results are pass/fail from 1–3 runs, tokens and cost are not recorded, and free models come and go.

## Goal

For one capable model, find how the budget affects **success, cost and time** on tasks whose real
working set is small, medium and large. Then pick the smallest Auto ceiling that loses nothing that
matters.

Non-goals: comparing models; tuning compaction thresholds (one variable at a time); testing the UI.

## The task family: one task shape, three sizes

All three sizes are the same task, generated at different sizes. Only the amount of material the
agent must understand changes, so differences between sizes come from memory pressure, not from
harder problems.

**Shape.** A generated project for a fictional system (a ledger/settlement service). It has:

- source modules (Python);
- a `docs/` spec, where each module's rules live (rates, rounding, ordering, error codes);
- data files the rules refer to.

The agent receives several messages in one chat, as in a goal run:

1. Implement a feature that touches every module, following the spec.
2. A follow-up that changes one rule from message 1 and adds a second feature.
3. A follow-up that asks for a command-line report built from all modules.

**Why the spec must actually be read:**

- The facts are generated per seed (names, constants, edge-case rules), so the model cannot guess
  them from general knowledge.
- Each module's rule appears only in its own spec page.
- The final grading needs every rule at once, so forgetting an early page fails a test.

**Sizes,** measured in tokens (bytes ÷ 4) and asserted by the generator's tests:

| Size | Material to read | Modules | Expect compaction at |
|---|---|---|---|
| Small | ~30k | 6 | only very small budgets |
| Medium | ~150k | 30 | 64k–128k |
| Large | ~450k | 90 | up to 400k |

The generator is deterministic per seed. Each trial uses a different seed, so a run cannot be
memorised and luck spreads evenly across budgets.

## Grading

- Hidden tests, run sandboxed as in `evals/hard`, reported per stage (message 1, 2, 3) and per
  module.
- The agent must write its own tests, and they must pass.
- **Harness self-tests** (automated, offline):
  - the reference solution passes every stage at every size;
  - planted defects (one wrong rule, a forgotten earlier rule, a missing report) fail exactly their
    stage;
  - the generator is deterministic, and its sizes fall within their bands.

## Arms and protocol

- **Budgets:** 64k, 128k, 200k, 400k, 1M, each clamped to the model's window.
- **Sizes:** small, medium, large.
- **Runs:** at least 3 per budget and size; more on the large size if noise is high.
- **Path:** the real app server and CLI (`--via app`). This needs the app to accept budgets
  above 256k, which comes with the context-auto change. Until then, `--via runner` for a pilot.
- **Model:** one pinned, capable model, chosen by a pilot.
  - A preflight request checks it is available before each round.
  - A round aborts if the model disappears mid-way.
- **Fairness:** arms run interleaved (not all of one budget first), so time-of-day provider load
  spreads evenly.

## Measures (per run, then mean ± sd per budget and size)

- pass rate and stages passed (primary);
- input tokens (with the cached share), output and reasoning tokens;
- estimated cost from models.dev prices, labelled ≈;
- wall time;
- requests, compactions, re-read share after compaction (`evals/compaction/redo.py` logic);
- failures by kind: provider error, cut-off reply, turn limit, wrong answer.

## Deciding the Auto ceiling

1. For the large size, find the smallest budget whose pass rate is within one standard error of the
   best budget.
2. Among budgets that tie on pass rate, prefer the cheaper and faster one.
3. If small and medium tasks pass at every budget of 128k and above, that confirms the ceiling only
   matters for large work.

Report it as a benchmark audit, with the full table and the chosen ceiling.

## Cost and time

- The large size at 1M can send several hundred thousand tokens per request.
- A pilot of one run per size at 200k measures the real per-run tokens before the full round.
- The full round is roughly 45 runs, which is many hours. Spread it across allowance windows, or use
  a free model if the pilot shows one is capable.

## Build plan (after approval)

1. Generator and reference solution, with self-tests (`evals/budget/`).
2. Hidden-test grader per stage, reusing `evals/hard` sandboxing.
3. A `--budgets` round runner, token and cost capture, and an interleaved schedule.
4. Pilot: one model, one run per size at 200k. Review the results before the full round.

## Decisions

- **Project:** generated, not a real open-source codebase (controlled sizes, no licence issues).
- **Spend:** paid OpenCode Go models may be used only when they are cheap and under hard spend
  control. The round runner reads each model's price (`cost`, $ per million tokens) from models.dev
  provider `opencode-go` and:
  - refuses a model whose input price is above `--max-price` (default $0.20/M);
  - stops scheduling new runs once the spend so far plus the next run's estimate would pass
    `--max-round-cost` (default $5);
  - stops a run that passes `--max-run-cost` (default $1.50), grades what exists and marks it
    `stopped: cost`.
  Free models (cost 0) are unaffected. Costs are estimates (≈) from the usage the provider reports.
