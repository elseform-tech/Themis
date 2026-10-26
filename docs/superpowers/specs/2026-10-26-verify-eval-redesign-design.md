# Verification eval, redesigned: realistic projects that stress the harness

## Why

The first `evals/verify` tasks fit in one file and one screen. A cheap model got 24/24 and then
9/9 on the "hard" ones, so correctness never moved. These tasks test the model, not Themis. Real
work spans a project too big to hold at once: the agent has to find its way around, read more
than its window holds, keep track across checkpoints, take a change of plan mid-task, and still
check its work at the end. That is where the harness can fail, and where an eval earns its keep.
The user's direction (2026-10-26): "the evals should span multiple files … simulate realistic
situation, putting stress and pressure on the harness is what makes evals powerful."

## Goals

- **Realistic projects.** A generated, multi-package codebase of about 40–60 files, about
  250–400k characters in all: models, storage, services, a CLI, a small web front end, tests,
  docs, sample data. Too big for the run's context budget, so it must be navigated, not read
  whole.
- **Pressure on the harness.** A small context budget (default 32k), so every task crosses
  several checkpoints; long files and logs worth reading; a follow-up message mid-task that
  changes a requirement; tasks needing many reads and writes across files.
- **Correctness that can move.** Hidden checks score the share of requirements met, not
  pass/fail, plus regressions in the project's existing tests. A task is hard because the cause is
  far from the symptom, or because several requirements interact, not because a puzzle is clever.
- **Verification measured, as now.** A check after the last change, claims without one, false
  claims, a bug reproduced first, tests that catch the bug, the browser on web changes, evidence
  for lookups.
- **Harness health measured.** Failed checkpoints, re-reads after a checkpoint, writes redone,
  tool errors that are ours, a follow-up lost after a checkpoint. A trend per model, scenario and
  budget, like the soak: nothing is pass or fail; a flag asks a person to look; branches are
  judged by A/B within a budget.
- **Cheap enough to run after each merge.** About $0.05–0.20 per run on a cheap model, with the
  existing per-run and per-round cost caps.

## Non-goals

- Not a model leaderboard. One cheap model is the default; others only for comparison.
- Not the goal soak. Goal mode, long autonomy and stall loops stay in `evals/soak`; this eval is
  single requests, sometimes with a follow-up.
- No real third-party services. Web lookups use the bundled SearXNG and public docs only.

## The project generator

`evals/verify/generate.py` writes a project deterministic per (seed, scenario): **Ledgerly**, a
small invoicing and inventory service in Python with a static web front end.

```
ledgerly/
  core/money.py  core/dates.py  core/ids.py         shared helpers (rounding, periods, ids)
  models/customer.py  invoice.py  product.py  stock.py
  storage/json_store.py  migrations.py  index.py
  services/pricing.py  tax.py  discounts.py  billing.py  inventory.py  reports.py
  cli/main.py  cli/commands/*.py                    5–7 command modules
  api/handlers.py  api/routes.py                    JSON handlers the web page calls
web/index.html  web/app.js  web/api.js  web/render.js  web/style.css
tests/ (25–40 tests across modules, all passing as given)
docs/ARCHITECTURE.md  docs/CLI.md  docs/CHANGELOG.md  docs/decisions/*.md
data/sample_invoices.json  data/rates.csv  logs/app.log (long, realistic)
```

Seeds vary names, numbers, file order and filler (comments, docstrings, docs prose, log volume),
so a model cannot memorise a project, and the same scenario can be rerun on fresh variants.
Filler is realistic: real-looking docs, decision records and logs, not lorem ipsum.

## Scenarios (one task each; each plants its own fault or gap)

| Scenario | What the user says | Why it is hard | Hidden check |
| --- | --- | --- | --- |
| `trace-rounding` | "The monthly report total is a cent off the invoices." | Symptom in `reports.py`; cause in `core/money.py`, used by 3 services. A patch at the symptom passes the visible test but breaks billing. | Hidden tests across reports, billing, tax; share passing |
| `feature-discount-codes` | Add discount codes: a model field, storage migration, pricing rule, a CLI flag, report column, docs. Six numbered requirements. | Six coordinated edits in six packages; old data must still load. | One hidden test per requirement plus migration of old data |
| `refactor-split-billing` | Split `services/billing.py` into two modules without changing behaviour. | 15+ call sites; behaviour must not change. | All original tests plus hidden behaviour tests; no import of the old name |
| `run-to-see` | "The import of `data/sample_invoices.json` drops some invoices." | Code looks right; only running the CLI on the data shows CRLF dates in another time zone. | Hidden import count and values |
| `web-mismatch` | "The invoice page shows NaN for totals." | The front end expects `total_cents`; an API handler returns `total`. The fix spans JS and Python; the browser shows it. | node check of `render.js` with a recorded API response, plus a Python handler test |
| `lookup-deprecation` | "Our date parsing warns about a deprecated call on Python 3.12+; fix it the way the docs recommend and cite them." | The fix is in `core/dates.py` and two callers; the right replacement is in the official docs. | Hidden tests, no deprecated call left, answer cites a page it opened |

Every scenario can carry a **mid-task follow-up** (`--followup`): queued about halfway through
(after N tool calls), it changes one requirement ("also keep the old CLI flag working",
"round half to even, not half up"). The hidden check includes the change, so a follow-up lost
across a checkpoint shows up as a missed requirement.

## Request pressure (added 2026-10-26)

The user's direction: stress "user requests related and unrelated … for example changing the
direction of the fix, or add tests or evals". Each scenario can run with a **pressure profile**,
a scripted sequence of extra messages, each sent either steered into the running turn (after N
tool calls) or queued for after it, and each with its own hidden check:

| Kind | Example | Checked |
| --- | --- | --- |
| change of direction | "don't patch the import; fix it in core/money.py so every caller gets it" | the new fix is in place and the superseded one is gone |
| tightened requirement | "round half to even, not half up" | the final code follows the change, after any checkpoint |
| add tests | "also add a regression test for this" | a test that fails on the original code exists |
| add an eval/check | "add a script that checks every invoice in data/ against its total" | the script exists, runs, and catches a planted mismatch |
| unrelated question | "what does docs/decisions/0003 say about time zones?" | the answer matches the file; the main task still completes |
| unrelated small fix | "`stock show` crashes on an unknown SKU" | both fixed, neither breaks the other |

Profiles: `none`, `related` (direction, requirement, tests), `unrelated` (question, small fix),
`mixed` (all). The profile is a series and A/B dimension like the budget. Extra measures:
`requests_completed` (share), `superseded_left` (work from an abandoned direction still present),
`request_lost_after_checkpoint`, and `regressions` in work done before a later request.

## Pressure settings

Per run, in the scratch `runtime.jsonc`: `context_token_budget` from `--budgets` (default
`32000,64000,auto`, like the soak), so the smaller budgets cross several checkpoints and auto shows
the unpressured baseline. **The budget is a trend and A/B dimension** (decided 2026-10-26): every
record carries it, a series is (model, scenario, budget), and A/B arms (`--label`, `--themis`)
are compared within the same budget and seed, never pooled across budgets; Custom mode with the existing
allowlist (OS sandbox on, never `shell_unsandboxed`); scripts allowed (`python3`, `node`),
because real projects use them. The project is a git repository with a few earlier commits, so
history and blame are available as in a real repo.

## Measures

| Area | Measure | Kind |
| --- | --- | --- |
| correctness | `requirements_met` (share of hidden checks passing, 0–1) | model |
| correctness | `regressions` (given tests that passed before and fail after) | model |
| correctness | `followup_applied` (when sent) | model |
| verification | `verified_after_last_change`, `unverified_claim`, `false_claim` | model |
| verification | `reproduced_first`, `test_catches_bug`, `tests_added`, `browser_checked` | model |
| evidence | `searched`, `evidence_cited`, `reason_correct` (lookup) | model |
| harness | `failed_checkpoints`, `tool_errors_ours`, `tool_errors_unclassified`, `failed_run` | harness |
| memory | `checkpoints`, `rereads_after_checkpoint`, `writes_redone`, `followup_lost_after_checkpoint` | model / reported |
| health | `tool_calls`, `requests`, `cost_usd`, `wall_minutes` | reported |

`false_claim` becomes graded: claiming success with `requirements_met` below 1. Tool-error
classes reuse `evals/soak/tool_causes.py`; checkpoint and re-read measures reuse the soak's
graders where they fit (moved to `evals/kit` if shared).

## What stays, what goes

- Stays: the runner, trend, report, cost caps and Custom-mode rules in `evals/verify` and
  `evals/kit`; the claim and evidence graders.
- Goes: the seven single-file tasks. The lookup and web ideas survive inside the project
  scenarios. Their history lines stay in git for reference.

## Order of work

1. Generator and two scenarios (`trace-rounding`, `feature-discount-codes`) with hidden checks
   and reference fixes; self-tests that each hidden check fails as generated and passes when
   fixed, and that the given tests pass as generated.
2. Pressure settings and the harness measures (checkpoint, re-read, follow-up graders).
3. The remaining four scenarios, then the request-pressure profiles (related, unrelated, mixed).
4. A pilot round (main vs the verify-habit branch, 2 seeds × 6 scenarios × the three budgets)
   to check that correctness now varies and the costs fit; tune project size if runs are too
   cheap or too dear. Report per budget, A against B.

## Left to the builder

The exact Ledgerly code and filler, the follow-up timing rule, and the per-scenario reference
fixes. Keep generated projects reviewable (a `--write DIR` option to look at one).
