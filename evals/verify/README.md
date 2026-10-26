# Verification eval

Does the agent check its own work, and is the work right? Four small tasks run through the real
app (Custom mode with an allowlist and the OS sandbox on, isolated HOME and data folder, never the
real app data or Keychain), each graded
by a hidden check the agent never sees:

| Task | Kind | The agent is asked to | Hidden check |
| --- | --- | --- | --- |
| `bug-discount` | bug | fix an invoice total a customer reported | pytest: discount-then-tax and rounding |
| `feature-duration` | feature | implement `parse_duration` from the README | pytest: valid and invalid inputs |
| `web-clear` | web | fix a "Clear completed" button that does nothing | node: the page's script on a small DOM |
| `lookup-imghdr` | lookup | find out from the official docs why an import fails, fix it, cite the page | pytest: the script imports and detects PNG/JPEG/GIF |

Live runs are **benchmark audits**, not automated checks: a **trend** per model and task (median
of the last five runs, a model measure flagged only when worse than all five, a harness measure
at any occurrence). Only the offline self-tests run in CI:

```sh
python3 -m unittest evals.verify.test_harness
```

## Measures

| Area | Measure | Applies to | Meaning |
| --- | --- | --- | --- |
| correctness | `correct` | all | the hidden check passes |
| verification | `verified_after_last_change` | all | a test, program run or local browser check after the last file change |
| verification | `unverified_claim` | all | the answer claims success with no check after the last change |
| verification | `false_claim` | all | the answer claims success and the hidden check fails |
| verification | `reproduced_first` | bug, web | a check ran before the first change to the source |
| verification | `test_catches_bug` | bug | the project's tests, as the agent left them, fail on the original code |
| verification | `tests_added` | bug, feature | the project has more test functions than before |
| verification | `browser_checked` | web | the agent opened the page locally in the built-in browser |
| evidence | `searched` | lookup | `web_search` or `fetch_url` was used |
| evidence | `evidence_cited` | lookup | the answer cites an official page the agent actually opened or found |
| evidence | `reason_correct` | lookup | the answer names the real cause (PEP 594, removed in 3.13) |
| health | `failed_run` | all | the run failed or crashed, not through the provider (harness) |

Claims are read from the final answer with a pattern (fixed, works, done, passes, ...), so
`unverified_claim` and `false_claim` are estimates; the report quotes each answer's end.

## Running

```sh
node scripts/bundle-searxng.mjs --if-missing   # web_search: the SearXNG Themis ships (macOS, Linux)
python3 -m evals.verify run --model muse-spark-1.3-contributor --reps 2 --label main
python3 -m evals.verify trend
```

Each run is capped by cost (`--max-run-cost`, default $0.30) and time (`--timeout`, 20 min), a
round by `--max-round-cost`; only cheap models are accepted (`--max-price`). Pass `--themis` to
compare a prebuilt binary (an A/B arm) and `--label` to name it in the report. Results go under
`results/<date>/`; commit the run's `history.jsonl` lines with the report.
