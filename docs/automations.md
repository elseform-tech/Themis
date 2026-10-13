# Automations

Create an automation with **Instructions**, **Repeat**, and **Time**. Daily,
Weekdays (Monday through Friday), and Weekly schedules use local calendar time.
The time zone defaults to the computer's IANA time zone. Advanced contains the
weekly day, time zone, optional name, destination, model, and enabled state.
Without a name, the first line of the instructions supplies one.

Continuing a chat uses that chat's current model and reasoning effort. A new-chat
automation stores its configured model and effort. Skills can be selected inside
the instructions using the prompt's `/` picker.

## Schedule behavior

A calendar schedule persists alongside `next_run_at` in `automations.json`:

```json
"schedule": {
  "repeat": "weekdays",
  "time": "09:00",
  "timezone": "Europe/Berlin",
  "weekday": 0
}
```

`weekday` uses Monday = 0 through Sunday = 6 and affects Weekly schedules only.
`interval_mins` remains in the stored record for backward compatibility; it is
used when `schedule` is absent or null. Editing an old automation retains
**Interval (legacy)** until another repeat option is selected.

The scheduler evaluates IANA time-zone rules with `chrono-tz`. A daily 09:00
Berlin run remains at 09:00 across daylight-saving changes. If a selected time
falls inside a spring-forward gap, the occurrence moves to the first valid
minute after that gap. An autumn repeated time runs at its earlier occurrence,
once on that local day. If a time zone skips a whole date, that date is skipped.

The local app server must be running for scheduled execution. On restart, a
persisted future occurrence stays unchanged; an overdue occurrence is deferred
about one minute by the existing boot recovery policy. Missed repetitions are
not replayed individually. Completion and failures select the next future
calendar occurrence; fixed intervals advance from their scheduled time (see
Watches).
The global scheduler switch and each automation's enabled state control automatic
runs. Run now remains available while either is paused.

## Watches (short intervals the agent manages)

A watch is an automation that repeats every few minutes and ends itself. The
agent creates one when you ask it to keep an eye on something ("watch
status.txt every 2 minutes and tell me when it says DONE, then stop watching").
It uses `manage_automations` on its own chat; you stop it from Workspace Tasks
(Stop or Pause).

**No approval for the agent's own chat.** In Custom mode too, the agent may
create, update, enable, disable, complete and delete automations that continue
the current chat without a prompt. The check runs inside the tool
(`tool_approves_itself`), so the dispatcher neither asks nor arms a permit.
Another chat's automations, and every other management tool, keep the normal
approvals. Deny rules still apply to a watch's `check_command`.

**Explicit timing.** Exactly one of:

- `interval_mins` 2 to 60 (optionally with `repeat: "interval"`), every N
  minutes; or
- a calendar schedule: `repeat` `daily`, `weekdays` (Monday to Friday) or
  `weekly` with `time` (HH:MM), `timezone` (IANA, `UTC` when omitted) and
  `weekday` (0 = Monday), either flat or as `schedule {...}`.

Mixing the two is rejected with an error instead of silently ignoring one.
Automations created in the UI keep accepting any legacy interval of at least
1 minute.

**Limits and backstops.** Fields are stored on the automation and optional for
user-created ones:

| Field | Meaning | Default for agent interval watches | Limit |
| --- | --- | --- | --- |
| `expires_at` / `expires_in_mins` | removed at that instant ("expired") | now + 120 min | at most 24 h ahead |
| `max_runs` | executions, including checks that start no model run ("run limit") | 60 | 1 to 1000 |
| `stop_when` | short plain-language goal shown in Tasks | none | 200 characters |
| `check_command` | argv array, see below | none | 16 arguments |

Three failures in a row (a failed run, or a check that cannot start, times out
or is denied) remove an agent watch ("failed 3 times") and pause a user's own
automation, which then shows `Paused · failed 3 times`. A watch whose chat was
deleted is removed ("chat deleted"). Agent calendar automations carry no default
expiry or run limit. Every removal is recorded with a reason in
`automation_removals.json` (newest 200) and logged as an `automation_removed`
diagnostic with the id and reason only. The limits are fixed constants, not
runtime configuration.

**`check_command`.** Before each due run the scheduler runs the argv through the
same shell runner as the `shell` tool (no shell string, scrubbed environment,
process group, 30 s timeout, output capped at 32 KiB), in the project root and
without any model call. Its exit code and output are hashed; the model starts
only when the hash differs from the last handled one (and on the first look),
and receives the new output as data. A non-zero exit code is a result, not a
failure (`grep -q DONE status.txt` works). The baseline moves only after a
successful model run. Only an approval-rule deny (Custom mode) blocks a check;
YOLO bypasses the rules. Each check logs a `watch_check` diagnostic with
`changed` and the exit code, never the output.

**Scheduler.** The scheduler ticks every 10 s. An interval watch runs from the
scheduled time, not from completion: the next slot is the previous scheduled
time plus the interval, missed slots are skipped, and a manual Run now restarts
the interval from now. A 2-minute watch therefore fires within about 10 s of
each slot. A tick skips an automation whose previous run or check is still in
progress, respects the concurrency limit and the global automations switch, and
does not advance the schedule when it cannot start.

**Tool results.** Every `create`, `update`, `enable` and `disable` result
echoes the stored record including `schedule_summary` ("every 2 min"),
`interval_mins`, `schedule`, `expires_at`, `max_runs` and `next_run_at`, so the
agent reports facts. `complete` (the agent marks its own watch done) removes it
with reason "completed by agent"; a running watch's prompt carries its id and
`stop_when`, so the agent can call `complete` directly. `id` may be omitted when
the chat has one automation.

## Origin and the chat Tasks view

Each automation records `origin`: `user` (default; omitted from JSON and for
records created before this field) or `agent` when the agent created it through
`manage_automations`. Updates keep the original origin.

`get_thread_tasks {threadId}` (also `themis call get_thread_tasks`) returns the
feed behind the Workspace **Tasks** tab and the header task indicator for one
chat: `run` (`running`, `awaiting_approval`), `automations` that target the chat
(`enabled`, `origin`, `schedule`, `next_run_at`, `last_run_at`, `running`) and
up to 10 `recent` finished runs (`ok`, `seen` once the review item is no longer
pending, `summary`). The UI refreshes it on existing run, approval, tool,
automation and review events. Stop in Tasks uses `stop_thread`; Pause uses
`set_automation_enabled` with `enabled: false`. Neither undoes completed work.
Only automations that continue a chat (`target_thread_id`) appear there. The
feed also carries each automation's watch fields (`stop_when`, `expires_at`,
`max_runs`, `check_command`, `stopped_reason`), `removed` (up to 10 automations
of the chat that ended: `reason`, `removed_at`, `origin`, `stop_when`) and an
`origin` on every finished run. A review item records the automation's name and
origin when the run starts, so a past run keeps its name and "by agent" tag after
the automation is deleted (older items fall back to the title without its
timestamp).

## Verification

`cargo test -p themis-desktop --lib schedule_tests` covers calendar recurrence,
weekdays, weekly selection, invalid settings, and both Berlin DST transitions.
`calendar_cli_e2e` covers actual `themis call` requests through the shared local
server, validation failure, persisted restart state, and legacy records.
`calendar_run_e2e` covers a mocked provider failure, subsequent calendar time,
review history, and restart persistence. UI form mapping and legacy edit behavior
are covered by `web/src/screens/automationForm.test.ts`.
`thread_tasks_cli_e2e` covers `get_thread_tasks` and pausing through `themis call`
against the shared server. `thread_tasks_report_live_automations_and_finished_runs`
(unit) covers live, finished and per-chat filtering, and the agent-origin
assertions live in the `manage_automations` management-tool test. UI grouping,
ordering, the indicator name and Stop/Pause dispatch are covered by
`web/src/screens/threadTasks.test.tsx`.

Watch behavior is covered by `agent_watch_e2e` (scripted provider: an unchanged
check makes zero model calls and a changed one exactly one; the agent creates a
watch in default Custom mode with no approval event and later completes it; the
expiry, run limit, three-failure and deleted-chat backstops; deny rules at
creation), `agent_watch_cli_e2e` (`themis call` with watch fields, validation
errors and the removal reason), unit tests for interval slot math, limit
validation, the mixed-timing rejection, echoed values and the approval boundary
(`own_chat_needs_no_approval_but_other_chats_still_do`), and the Tasks rows in
`threadTasks.test.tsx`. Live evidence:
[agent-watches-2026-10-13](verification/agent-watches-2026-10-13.md).
