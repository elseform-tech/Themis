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
calendar occurrence; fixed intervals retain their prior completion-based timing.
The global scheduler switch and each automation's enabled state control automatic
runs. Run now remains available while either is paused.

## Verification

`cargo test -p themis-desktop --lib schedule_tests` covers calendar recurrence,
weekdays, weekly selection, invalid settings, and both Berlin DST transitions.
`calendar_cli_e2e` covers actual `themis call` requests through the shared local
server, validation failure, persisted restart state, and legacy records.
`calendar_run_e2e` covers a mocked provider failure, subsequent calendar time,
review history, and restart persistence. UI form mapping and legacy edit behavior
are covered by `web/src/screens/automationForm.test.ts`.
