# Verification eval

Realistic work under pressure, through the real app: does the agent find the cause, make the change
across files, take changed instructions, and check its own work? Design:
`docs/superpowers/specs/2026-10-26-verify-eval-redesign-design.md`.

Each run gets a freshly generated **Ledgerly** project (`generate.py`): an invoicing and stock
service of about 100 files and 330k characters (core, models, storage with migrations, services, a
CLI, a JSON API, a web page, tests, docs, sample data, a long log), too big for the run's context
budget. A scenario plants one fault or gap where its cause is away from its symptom; hidden checks
the agent never sees score the share of requirements met.

| Scenario | The user says | Hard because |
| --- | --- | --- |
| `trace-rounding` | some amounts come out a cent short | the cause is a shared helper used by the importer, the API and the CLI |
| `feature-discount-codes` | add discount codes (six numbered requirements) | model, migration, pricing, CLI, CSV and docs must all change |
| `refactor-split-billing` | split `billing.py` without changing behaviour | 15+ imports in code, tests and docs |
| `run-to-see` | the import drops some invoices | the code looks right; only running it on the sample data shows why |
| `web-mismatch` | the page shows NaN | the API and the page disagree on field names |
| `lookup-deprecation` | fix a deprecation as the docs say, cite them | an aware/naive datetime trap in stored data |
| `interleaved-requests` | add `reports top`, then an urgent unrelated fix, a question, and "now finish it" | the feature must survive interruptions and checkpoints; each step is checked |
| `accumulating-requests` | `customers show`, then eight more requests piling up (added requirements, a change of direction, unrelated fixes, a test, "update the docs for everything today"); 16k and 32k by default | early requests must still be done at the end, under heavy compaction |

**Request pressure** (`pressure.py`, `--profiles`): scripted messages sent while the agent works,
steered into the turn or queued after it: `related` (a change of direction, a tightened
requirement, "add a regression test"), `unrelated` (a question about the docs, a separate small
bug), `mixed` (both), `hold` ("stop changing files and wait for me", which the agent should
obey: `held` scores no file written after it, and a reply), or `none`.

**Budget and profile are trend and A/B dimensions**: a series is (model, scenario, budget,
profile); `--budgets` defaults to `32000,64000,auto`. Compare branches with `--themis` and
`--label`, then `ab`, which shows each measure's mean per budget and profile.

Live runs are **benchmark audits**, not automated checks: nothing is pass or fail; a model measure
is flagged when worse than all of its series' last five runs, a harness measure at any
occurrence. Only the offline self-tests run in CI:

```sh
python3 -m unittest evals.verify.test_harness
```

## Measures

| Area | Measures |
| --- | --- |
| correctness | `requirements_met` (share of hidden checks), `regressions` (project tests failing at the end) |
| pressure | `requests_completed`, `superseded_left`, `request_lost_after_checkpoint`, `unrelated_answered`, `held` |
| verification | `verified_after_last_change`, `unverified_claim`, `false_claim`, `reproduced_first`, `test_catches_bug`, `tests_added`, `browser_checked` |
| evidence (lookup) | `searched`, `evidence_cited`, `reason_correct` |
| memory | `checkpoints`, `rereads_after_checkpoint` |
| harness | `failed_checkpoints`, `tool_errors_ours`, `tool_errors_unclassified`, `failed_run` |

Claims are read from the final answer with a pattern, so the claim measures are estimates; the
report quotes each answer's end.

## Running

```sh
node scripts/bundle-searxng.mjs --if-missing     # web_search: the SearXNG Themis ships (macOS, Linux)
python3 -m evals.verify run --model muse-spark-1.3-contributor --profiles none,mixed --seeds 1,2 --label main
python3 -m evals.verify run ... --themis path/to/branch/themis --label my-branch
python3 -m evals.verify ab --a main --b my-branch
python3 -m evals.verify.generate DIR --scenario run-to-see --seed 3   
```

Runs use Custom mode with an allowlist (the OS sandbox stays on; never `shell_unsandboxed`), an
isolated HOME and data folder, and per-run (`--max-run-cost`, default $0.50) and per-round
(`--max-round-cost`) cost caps; only cheap models are accepted (`--max-price`).
