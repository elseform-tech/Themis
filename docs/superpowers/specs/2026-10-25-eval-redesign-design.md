# Eval redesign: test the harness, not the model

Status: proposal for review. Nothing built yet. Decisions from review on 2026-10-25: trend eval
instead of pass/fail; checkpoint rule retention is a tracked model measure.

## Why

The live evals mostly measure what a model can do: the post-merge check's spreadsheet task fails
on how a model reads an unstated CSV line-ending rule, which says nothing about Themis. What we
need to know after a change is whether **our** machinery holds up over long work:

- **Goal mode**: turns continue by themselves, stop when the goal is reached, and pause, resume
  and edit work.
- **Compaction**: every checkpoint succeeds and shrinks the context, and the goal, the rules and
  the progress survive it.
- **Tools**: tools behave, report errors clearly, and ask for approval exactly when they should.

There are seven eval suites (about 17k lines of Python plus five Rust runners). Most overlap,
none drives goal mode, and some test model skill. This redesign replaces them with fewer, sharper
checks and removes the rest.

## Goals

- Catch regressions in goal mode, compaction and tool behaviour, automatically where possible.
- One live check after each merge, tracked as a trend: each run is compared with the recent runs,
  so a regression shows as a measure leaving its usual range, and a breakage of our code is
  flagged apart from model noise.
- Fewer suites and less code than today.

Non-goals: ranking models; measuring coding skill; UI testing (covered by browser/native checks).

Anti-goals: a verdict that one lucky or unlucky model reply can flip; a report that needs a person
to read transcripts to see what changed; more suites than today.

## Two layers

### 1. Automatic (CI, scripted provider, no real model)

A scripted model (wiremock, as the existing e2e tests use) makes the agent's moves exactly, so
these tests check only our code and run on every PR at no cost.

Much is already covered (see the inventory in the appendix). New tests fill the gaps:

| Test (crate `tests/`) | What it proves |
| --- | --- |
| `goal_soak_e2e` (desktop, through the app server and CLI) | A goal runs at least 12 turns at a 16k budget, crossing at least 4 checkpoints. Each turn the scripted model writes the next item file; the scripted check answers `not_yet` until the last item, then `reached`. Checks: every turn after the first starts by itself; every checkpoint succeeds and the next request is smaller than the one before it; the goal text and a rule from the first message are in every request after a checkpoint; no item is written twice (no replayed writes); the goal stops on `reached` with no further turn; pause during a turn holds after it, resume continues; `get_goal` elapsed time grows only while running. |
| `checkpoint_cannot_fit_fails_clearly` (core) | The "checkpoint and active request exceed the context budget" path (hit live on 10-18) fails with that error, keeps the originals and the working context, and the next run still works. |
| `apply_patch_through_the_runtime` (core) | apply_patch end to end through a run: a valid patch applies once, an ambiguous one fails with an error that names the problem, and a retry with the fix applies. Today it is only unit-tested. |

These are deterministic, so their pass/fail is the regression signal.

### 2. Live soak (after each merge, one cheap real model)

One eval, `evals/soak`, run through the real app server and CLI exactly as a person's goal runs:
a scratch HOME and data folder, a scratch git project, `/goal` started with `send_message`, events
from `themis watch`, the goal's status from `get_goal`.

**The job is easy but long**, so that model skill is not the bottleneck and the length forces
many checkpoints:

- A generated project with N small source files (default 40), each holding a few facts (name,
  owner, a number, a status). Generated per seed, so it cannot be memorised.
- The goal: for every source file, write `out/<name>.md` in a fixed format, then `out/INDEX.md`
  with a total, then run `python3 check.py` (provided) until it passes.
- Two rules appear only in an ordinary first message, sent before `/goal` (for example, every
  output file starts with `owner: <team>`, and items with status `retired` are listed in the index
  but get no file). The goal text itself does not repeat them: the goal is added to every request,
  so rules inside it would never depend on compaction. *(Builder-check call.)*
- Midway, a queued follow-up message changes one format detail for all items. The agent must
  update the files already written (`apply_patch` or edits), which tests steering and the queue
  in goal mode.
- Budget: 24k by default, so a 40-item run crosses roughly 10–20 checkpoints. `--budgets` adds
  arms (for example `24000,64000,auto`), which answers the Auto ceiling question that the budget
  eval was built for.

Every tool gets used naturally: `list_dir`, `read_file`, `write_file`, edits or `apply_patch`,
`shell` (the checker), and `git` (the job ends with a commit).

#### What it measures

The soak is a **trend eval**, not a pass/fail check (decided 2026-10-25): every run appends its
measures to a history, and the report compares the run with the recent ones. Each measure has a
kind that decides how it is read:

- **harness**: a breakage of our machinery (a crash, a failed checkpoint, a goal that does not
  continue). A single occurrence is flagged at once, since a scripted CI test should already have
  caught it; it is also tracked.
- **model**: depends on the model's replies; tracked as a trend and flagged only when it leaves its
  recent range (see "Trend and regression flags").

| Area | Measure | Kind |
| --- | --- | --- |
| Goal | Turns after the first start by themselves (judged up to the run's end, whatever ended it) | harness |
| Goal | After `reached`, no further turn starts | harness |
| Goal | The goal stays set and unchanged (`get_goal`), unless the run edits it | harness |
| Goal | Reached within the turn limit and spend cap | model |
| Compaction | Every checkpoint succeeds (no `failed` checkpoint, no budget-exceeded error) | harness |
| Compaction | Context after each checkpoint is smaller than before it | harness |
| Compaction | Share of first-message rules and the goal text kept in each saved checkpoint (`get_thread_history_page`) | model, trend *(decided 2026-10-25: the summary is model-written; CI proves our code hands the rules to the summarizer)* |
| Memory | Output files written after each checkpoint still follow both rules | model (reported per checkpoint) |
| Memory | Items redone (an output written again with no change asked), skipped or duplicated | model; a redo right after a checkpoint whose saved progress already listed the item is flagged as the checkpoint's share, reported but not failing *(builder-check call)* |
| Memory | Re-reads of already-finished sources after a checkpoint | model (trend) |
| Tools | Tool errors by cause (below) | harness for our causes |
| Health | The run ends in a known state: goal reached, goal paused as stuck (three turns without progress), or a spend/turn cap. A `failed` event, a crash, or a server that stops answering is harness; stuck and caps are model and reported. | harness / model |
| Health | Time, requests, tokens (cached share), ≈ cost | reported |

**Tool error causes.** Each failed tool call is classified from its tool, error code and message
(never from a model's opinion):

- *Ours*: a tool crashed or returned an internal error; a valid patch was refused; a sandbox or
  approval refusal of an action the run's mode allows; an error message missing the fact needed
  to recover (for example "file not found" without the path).
- *The model's*: invalid arguments, a path that does not exist, an ambiguous patch, a command
  that fails on its own.
- *Unclassified*: listed in the report with tool and code for a person to sort. A new pattern is
  added to the classifier, so this list stays short.

#### Calls made after the builder check

- Recall questions (an early fact, a rule, a count) are asked after the goal is reached, as
  ordinary messages; they are model measures. "No turn after `reached`" counts only turns the
  goal started, not turns from sent messages.
- An unclassified tool error is listed first in the report and tracked as its own count; the
  classifier is updated before the next round.
- One post-merge soak is **one run** at the default budget. The Auto-ceiling study is a separate,
  opt-in round (`--budgets`, three seeds per arm, under the per-round cap), reported per budget.

#### Trend and regression flags

- Each run appends one record to `evals/soak/history.jsonl` (committed with the live-check
  report): commit, date, model, budget, seed, and every measure. Runs lost to the provider (rate
  limit, outage, cut-off reply) are rerun once, then recorded as infrastructure and left out of
  the trend.
- A measure's **baseline** is the median of the last 5 recorded runs with the same model and
  budget. A run is flagged as a **regression** on a measure when it is worse than every one of
  those 5 runs (outside their range), or when a harness breakage occurs at all. With fewer than 5
  runs, the report shows values without flags.
- Changing the model, the job or its size starts a new series, so series never mix.
- The report is a benchmark audit, separate from automated checks: per measure, this run, the
  baseline and range, and a flag; a small sparkline of the last runs; cost and time. With
  `--budgets`, one table per budget.
- A flag asks a person to look (and, for a harness breakage, to add a scripted CI test that
  reproduces it). Nothing is "pass" or "fail".

#### Proving the soak catches what it should

Before the old suites are removed, the soak's graders are checked offline against recorded event
streams (a passing run, and runs with each harness failure planted), and the whole soak runs live
once on main and once on a throwaway branch with a planted regression (goal continuation turned
off; a checkpoint that drops the first message's rules). Each planted run must be flagged on the
matching measure against the main runs' history.

#### Cost and spend control

- Cheap models only (≤ about $0.20 per million input tokens; default `muse-spark-1.3-contributor`)
  under hard caps: per run $1.50, per round $5 (the budget eval's spend control, moved into the
  shared kit). A preflight checks the model is listed and priced.
- A 40-item run at 24k is expected to take 20–40 minutes; the pilot measures the real figure and
  sets N so that one post-merge soak stays under about 45 minutes.

## What stays, what goes

| Suite | Decision | Reason |
| --- | --- | --- |
| `evals/compaction` | **Keep, trimmed** | Checks what a summary keeps, offline and in CI. Drop cases that only stressed 16–24k budgets (midturn, redo) once the soak covers redo. |
| `evals/soak` | **New** | The live check. |
| `evals/kit` | **New** (shared helpers) | Isolated environment, sandbox wrapper, CLI calls, git helpers, spend control, evidence keeping. Moved from today's suites. |
| `evals/tools` | **Remove** | Measures first-try tool skill of a model. Tool behaviour moves to CI and the soak's tool causes. |
| `evals/hard` | **Remove** | Model skill (three.js, spreadsheet). |
| `evals/project` | **Remove** | Model coding skill. |
| `evals/horizon` | **Remove** after porting its graders (`rereads`, progress across turns, rule kept) into the soak. |
| `evals/longform` | **Remove** | Recall after compaction becomes three questions at the end of the soak (an early fact, a rule, a count), answered from the chat. |
| `evals/budget` | **Remove** | Its question becomes the soak's `--budgets`; its spend control moves to the kit. |
| Rust runners | Remove `tool_eval` (and `runtime/tool_eval.rs`), `longform_eval`, `redo_eval`; keep `compaction_eval` and `behavior_eval` for the compaction suite. | Only removed suites use them. |

Past results and reports stay in git history and `docs/verification/`. `docs/live-verification.md`
becomes: build; the desktop app by hand; the soak. AGENTS.md's live-check paragraph names the
soak instead of `evals.hard` and `evals.tools`.

## Order of work

1. CI tests (`goal_soak_e2e`, the checkpoint-cannot-fit path, apply_patch through the runtime).
   One PR; useful on its own.
2. `evals/kit` and `evals/soak` with offline grader self-tests. One PR.
3. Pilot: one live soak on main, one on a planted-regression branch; set N and the default
   budget from the pilot. Report as a benchmark audit.
4. Switch `docs/live-verification.md` and AGENTS.md to the soak.
5. Remove the old suites and runners in one PR, after step 3 has flagged both planted regressions. `redo_eval` is also used
   by `evals/compaction` (`redo.py`, its `__main__.py` and harness tests), so its redo cases go in
   the same PR; `.github/workflows/ci.yml` runs `evals.longform.test_harness`, which goes too.

## Left to the builder

- The exact generated facts and output format, and the checker script.
- Names of the CI tests and how the soak's scripted pieces share fixtures with the existing
  goal tests (`goal_queue_e2e.rs`, `crates/desktop/tests/common/`).
- The tool error classifier's patterns, starting from the tool error codes in `crates/core/src/tools.rs`.

## Appendix: what CI already covers

- **Goal**: `goal_queue_e2e.rs` (turn after turn until reached, stalls, pause/resume/edit,
  quota pause and model switch, resume on restart, the agent's goal tool, small-window note) and
  `goal_cli_e2e.rs` (CLI start, queue, read back, resume, remove). Nothing runs a goal past about
  four turns or across a checkpoint.
- **Compaction**: `runtime_e2e.rs` (forced checkpoints at 24k/200k/256k without replayed writes,
  overflow retry, second overflow fails with originals kept, evidence across checkpoints, stop
  during compaction, learned window) and `compaction_cli_e2e.rs`; recall in `recall_e2e.rs`.
  Not covered: the budget-exceeded error path; goal text or first-message rules surviving.
- **Tools and approvals**: unit tests per tool in `tools.rs`; through a run: shell, background
  tasks, browser, web, MCP, sandbox; approvals in `chat_grants_e2e.rs` and
  `shell_approval_e2e.rs`. Not covered: apply_patch through a run.
