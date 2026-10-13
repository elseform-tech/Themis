# Agent watches: live verification, 2026-10-13

Live evidence, separate from the automated tests. Isolated `HOME` and `--data-dir`
under a scratch directory, the debug `themis` binary from branch `codex/agent-watches`,
real model `longcat-2.5-preview-free` through `OPENCODE_KEY` (value never printed),
default **Custom** mode (no YOLO). Commands: `themis --data-dir D server start`,
`themis --data-dir D thread create PROJECT longcat-2.5-preview-free`,
`themis --data-dir D thread send ID "<prompt>" --json`, `themis --data-dir D call
get_thread_tasks`. The desktop window was not launched. Model calls are counted as
`context/request_usage` events in the isolated diagnostics log.

## 1. "Watch status.txt every 2 minutes and tell me when it says DONE, then stop watching"

Prompt, in plain words: "Please watch status.txt every 2 minutes and tell me when it
says DONE, then stop watching." (20:50:40 to 20:51:03 UTC, 4 model requests.)

- The agent first tried `check_command ["cat","status.txt"]`. In Custom mode that is a
  shell action that asks, and the unattended `thread send` cannot approve, so the call
  failed with "The user did not approve the check command, so the watch was not saved".
  This is the intended behavior after the security fix: nothing ran.
- The agent then created the watch **without** `check_command` (no approval prompt for
  the create itself; no `approval_request` events except an automatic read of the
  management skill). Result echoed: `schedule_summary` "every 2 min", `interval_mins` 2,
  `expires_at` +2 h, `max_runs` 60, `stop_when` "status.txt says DONE"; the agent reported
  exactly these facts.
- Because no check command was approved, each scheduled run is one model run that reads
  the file. So the "zero model calls while unchanged" criterion is **not met in this live
  scenario**; it is covered by the automated test
  `check_command_only_wakes_the_model_when_its_output_changes` (and the approval paths
  by `approval_is_bound_to_the_exact_argv_and_policy_is_rechecked`, which approve the
  exact argv). Live check-command gating with an interactive approval was not run
  (needs a terminal approval or a desktop click).

Timings (UTC; slots are the stored `next_run_at`):

| Slot | Run start | Offset | Model requests after |
| --- | --- | --- | --- |
| 20:52:58 | 20:53:00.5 | +2.5 s | 5 |
| 20:54:58 | 20:55:00.5 | +2.5 s | 6 |
| 20:56:58 | 20:57:00.6 | +2.6 s | 10 (run + completion) |

Runs started exactly 120 s apart (scheduled-time anchoring, 10 s tick). `status.txt`
was set to DONE at 20:55:31. The next run (20:57:00) read DONE and called `complete`
at 20:57:14, 1 m 43 s after the change (within one interval). Tasks showed
`removed: completed by agent` (origin agent, `stop_when` kept, `run_count` 2), the
automation list was empty, and the three finished runs kept the name "Watch status.txt
for DONE" with origin `agent` after deletion. Diagnostics logged `automation_removed`
with reason "completed by agent". Totals: 10 model requests (4 creating the watch, 1 per
scheduled run, plus the completion call).

## 2. Expiry backstop

Prompt: "Watch other.txt every 2 minutes for me, but only for the next 3 minutes, then
give up." Created at 21:00:13 with `expires_at` 21:03:13 and one run at the 21:02:13
slot (again the first `check_command` attempt was declined and retried without it).
The watch was removed at 21:03:20 (7 s after expiry) and Tasks showed `removed: expired`.
The agent's own summary said "up to 3 times", slightly inexact; the stored values
(2 min, expiry 21:03:13) were correct and were echoed.

## Automated results on this branch

`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`
passed; `cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten`:
547 passed, 0 failed; `npm --prefix web run lint`, `npm --prefix web test` (348 passed),
`npm --prefix web run build` passed.

## Not verified

Native desktop rendering of the Tasks rows (left to the lead), interactive approval of a
check command in the real UI, and live zero-model-call polling with an approved check
command.
