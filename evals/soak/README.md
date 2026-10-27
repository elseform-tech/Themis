# Goal soak

A long, easy job run through the real app in goal mode, so that **our** machinery is what gets
tested: goal turns continuing by themselves, checkpoints that succeed and keep the rules, tools
that behave. Design: "Layer 2: Live soak" in
`docs/superpowers/specs/2026-10-25-eval-redesign-design.md`.

Live runs are **benchmark audits**, not automated checks. They are a **trend**: each run is
compared with the recent runs of the same model, budget, job and size, and a flag asks a person to
look. Nothing is pass or fail. Only the offline self-tests run automatically (in CI):

```sh
python3 -m unittest evals.kit.test_kit evals.soak.test_harness
```

## The job (`items-v3`)

**Tools are limited (Custom approval rules in the run's `runtime.jsonc`, decided 2026-10-25).** In the
`items-v2` pilots the agent learned the generator's sentence templates with Python scripts and
finished 120 memos in 2–5 goal turns with 1–2 checkpoints, so the soak stressed nothing. The run
now allows only `read_file`, `list_dir`, `write_file`, `apply_patch`, `recall`, `set_goal`, `git`
the command `python3 check.py`, and `grep -L`/`-c`/`-rL`/`-rc`, which print only file names or counts
(added 2026-10-27 so a goal check asking for compact proof of a change to every file can get it;
no file contents, so items are still read one at a time); everything else (other commands, scripts,
`search_file`) is denied by default, so no approval ever waits. `FORMAT.md` tells the agent so. A denial of an
allowed tool is graded as ours; any other denial is the model reaching past the rules.


`generate.py` writes a project, deterministic per (seed, items). The first pilot (`items-v1`,
`key: value` sources) was finished in 2 goal turns by one Python script that parsed every file,
so v2 makes every item need the model's own reading:

- `src/<name>.txt` for N items (default 120) is a prose memo in varied, seeded wording. No fact is
  a `key: value` line, and each has to be resolved:
  - **owner**: the team that holds the item *now*, after a hand-over told in a sentence ("took it
    over from", "ownership passed from … to …"), sometimes next to a proposed move that was dropped;
  - **status** (`active`, `paused`, `retired`): implied by events ("switched off for good", "put on
    hold … nobody has switched it off", "running again since"), often after an earlier state;
  - **count** of records: in words, as a sum of two sites, as a corrected audit figure, or next to
    a figure that belongs to another system;
  - **purpose**: the keyword the one-line summary must contain, next to a sibling with another
    purpose;
  - **since** (asked for by the follow-up): the year the current status began, among other years.

  About 6,000 characters of background per memo (other teams, years, record counts, hand-overs and
  shutdowns of other things) pad it and defeat regexes. A first- or last-match regex, plain or
  phrase-aware, gets under 20% of items fully right (`ScriptShortcutTests`). Every memo stays under
  8,000 characters, so it fits one `read_file`.
- `FORMAT.md` says how each output and the index are written (it mentions neither rule), and
  `check.py` checks only structure (fields present, a count in digits, a known status, a sentence of
  summary, a year for `since`) and the index total, by a salted hash. It holds no fact and checks
  neither rule.

The chat, through the app server and CLI:

1. An ordinary first message with two rules (`prompts/rules.md`): every item file starts with
   `owner: <team>` (the current owner), and retired items are listed in the index but get no file.
   The rules appear nowhere else: not in the goal (which is added to every request, so it never
   depends on compaction), not in `FORMAT.md`, not in `check.py`.
2. `/goal` (`prompts/goal.md`): write every `out/<name>.md` and `out/INDEX.md`, run
   `python3 check.py` until it passes, commit.
3. Once half the active items have files, a follow-up is queued with `enqueue_message`
   (`prompts/followup.md`): add `- since: <year>` to every item file. The year comes from each
   memo, so the files already written need their memos read again and every file edited.
4. After `reached`, three recall questions as ordinary messages, to be answered from memory: an
   early fact (the first item's count), a rule, the number of retired items.

### Size

At the defaults (120 items, 6,000 filler characters) the material is about 196k tokens, about
1,650 per memo. A run adds roughly
twice the material to the context: one read of every memo, a second read of the memos written
before the follow-up, writes and edits, plus tool overhead.

The default budget is 64k (decided 2026-10-25). A first pilot at 24k checkpointed every ~5
requests and never wrote an output file in an hour: too little room to read a memo and write
its file between checkpoints. At 64k a checkpoint frees about 40k. The first
round with limited tools (five runs on main, `muse-spark-1.3-contributor`, 2026-10-25) took 13–39
checkpoints, 370–570 requests, 18–83 minutes and $0.33–0.58 per run, under the $1.50 cap; runs
are capped at 2 hours. How many goal turns this takes depends on how
much the model does per turn. `--items` and `--filler-chars` scale the job; changing either
starts a new series.

## Running

```sh
python3 -m evals.soak run --model muse-spark-1.3-contributor            # one post-merge run
python3 -m evals.soak run --budgets 24000,64000,auto --seeds 1,2,3        # the Auto-ceiling round
python3 -m evals.soak report evals/soak/results/DATE/soak-24000-1-1.json  # regrade a saved trace
python3 -m evals.soak trend                                               # latest run per series
```

- Each run gets a scratch HOME, TMP and data folder (`evals.kit.env.isolated_env`): only `PATH`,
  `OPENCODE_KEY` and the scratch folders reach the app, so no Keychain and no real app data are
  touched. The budget goes in the data folder's `runtime.jsonc` (`context_token_budget`, a
  number or `"auto"`) with the job's Custom approval rules; the app's own OS sandbox applies.
- `--themis` takes a prebuilt CLI; otherwise `cargo build -p themis-desktop --bin themis` runs.
- **Spend**: cheap models only (≤ $0.20 per million input tokens at the run's budget; `auto` is
  priced at the model's dearest context tier). A preflight checks the model is listed by
  models.dev and by the provider. Spend is metered live from `context_usage` events and the
  app's own usage ledger (`get_usage_summary`, which also counts goal checks and checkpoint
  summaries), and each count is the larger of the two. A run that passes `--max-run-cost`
  (default $1.50) is paused and stopped at once; a run starts only if the round's spend plus its
  estimate stays under `--max-round-cost` (default $5).
- Other caps: `--max-turns` goal turns (80), `--timeout` seconds (7200). With the goal running and
  the chat idle for `--idle-timeout` seconds (300), the run ends as `not_continuing`. The goal
  event the app repeats every minute while a goal runs does not count as activity.
- A run lost to the provider (its last run failed on a rate limit, outage, cut-off reply or
  quota) is rerun once with the same seed, then recorded as infrastructure and left out of the
  trend. A provider failure the goal recovered from, with later turns, does not lose the run.
- Each run writes its trace (`results/DATE/soak-BUDGET-SEED-ATTEMPT.json`), appends one record to
  `history.jsonl`, and the round writes `results/DATE/report.md`. Commit the history with the
  live-check report. `--keep DIR` keeps each run's scratch folders.

## What each measure means

Kinds: **harness** (our machinery broke: flagged at any occurrence), **model** (depends on the
model's replies: flagged only when worse than every one of the last 5 runs), **reported** (never
flagged).

| Area | Measure | Kind | Meaning |
| --- | --- | --- | --- |
| goal | `goal_continuation_breaks` | harness | The goal stayed running with no turn starting until the idle timeout (`not_continuing`); a cap that cuts a turn short is not a break. Goals never stop for lack of progress, so a run ends reached, waiting, at a cap, or broken. |
| goal | `turns_after_reached` | harness | Goal turns that started after the goal's status became `reached` (the app's one short turn telling the user, and recall messages, do not count). |
| goal | `goal_changed` | harness | `get_goal` polls, while the goal ran, that found no goal or a different text. |
| goal | `reached` | model | 1 when the goal was reached within the caps. |
| goal | `goal_turns` | reported | Goal turns started. |
| compaction | `checkpoints` | reported | `context_checkpoint` events. |
| compaction | `failed_checkpoints` | harness | `context_suspended`, a budget-exceeded error, or a run that failed while compacting. |
| compaction | `checkpoints_not_shrinking` | harness | Checkpoints whose next context (`context_budget.used_tokens`) is not smaller than the one before. |
| compaction | `rule_retention`, `rule_retention_min` | model | Mean and lowest share of the goal, both rules (and, after the follow-up, the format change) kept in each saved checkpoint (`get_thread_history_page`), by text patterns. |
| memory | `rule_violations_after_checkpoint` | model | Item files last written after a checkpoint that break a rule: no `owner:` first line, or a file for a retired item (per checkpoint in the report). A wrong team on the owner line is `owner_accuracy`, not a forgotten rule. |
| memory | `redone_items` | model | Full rewrites of an item file with no change asked (same side of the follow-up, no `check.py` run between). |
| memory | `redone_checkpoint_share` | reported | Of those, redos right after a checkpoint whose saved progress already named the item: the checkpoint's share. |
| memory | `skipped_items`, `duplicated_items` | model | Active items with no file; stray files and items listed twice in the index. |
| memory | `rereads_after_checkpoint` | model | `read_file` of a memo whose output was already written, in the same phase (before or after the follow-up, which needs every memo again), before an earlier checkpoint. |
| memory | `item_accuracy` | model | Share of active items whose final file has the right count, status and purpose keyword. |
| memory | `owner_accuracy` | model | Share of active items whose first line names the current owner exactly. |
| memory | `index_accuracy` | model | Share of items whose status in `out/INDEX.md` is right. |
| memory | `format_change_applied` | model | Share of active items whose final `since` year is right. |
| memory | `check_passes` | model | The harness's own copy of `check.py` (structure and total) passes on the final project. |
| memory | `recall_correct` | model | Recall answers that match and used no tools (0–3). |
| tools | `tool_errors_ours` | harness | Failed calls classified as ours (see below). |
| tools | `tool_errors_model` | model | Failed calls caused by the model. |
| tools | `tool_errors_unclassified` | harness | Failed calls no pattern matched; listed first in the report. |
| health | `health_breakage` | harness | The run ended `failed`, `crash`, `server_hang` or `not_continuing` (not `reached`, `stuck`, `waiting` or a cap). |
| health | `failed_runs` | harness | Runs that failed for a reason that is neither the provider nor our stop. |
| health | `wall_minutes`, `requests`, `input_tokens`, `cached_share`, `cost_usd` | reported | Time and ≈ spend. |

**Tool error causes** (`tool_causes.py`), from the tool, its error text and the approval mode,
never from a model's opinion: *ours* is an internal error or crash, an approval or sandbox
refusal of an action YOLO allows inside the project, a valid patch refused, or an error with no
path or value to act on; *the model's* is invalid arguments, a missing path, a mismatched or
ambiguous patch, a command that failed on its own. Anything else is *unclassified*: sort it and
add a pattern before the next round.

## Reading the report

Per run: the end state, cost and time, unclassified and our tool errors first, then one row per
measure with this run's value, the baseline (median of the last 5 runs of the series), their
range, a flag and a sparkline of the recent runs; then a per-checkpoint table (what the saved
summary kept, and rule violations in files written after it). With `--budgets`, one section per
budget.

- **breakage** on a harness measure: our code misbehaved. Find it in the trace, then add a
  scripted CI test that reproduces it (`crates/desktop/tests/`).
- **regression** on a model measure: worse than all of the last 5 runs. Look at the trace before
  blaming the change: one run can be unlucky, which is why nothing fails on it.
- With fewer than 5 earlier runs in a series, model measures show values without flags.
- Changing the model, the job (`generate.JOB`) or the item count starts a new series.

## Offline self-tests

`test_harness.py` checks the generator (determinism, sizes, the rules only in the first
message, no fact as a `key: value` line, `check.py` revealing no fact), that regex extractors get
most items wrong, every grader on synthetic event streams (a clean run, and one per planted
breakage: a goal that stops continuing, a turn after reached, a failed checkpoint, a context that
does not shrink, a redo after a checkpoint, a tool's internal error), the trend logic (fewer than
5 runs, series split, infrastructure left out), and the runner end to end against
`fake_themis.py`, a stand-in CLI with a scripted agent (a clean run, a run stopped by the spend
cap, a goal that stops continuing while the app repeats its minute goal event). `evals/kit/test_kit.py` covers the shared kit.
