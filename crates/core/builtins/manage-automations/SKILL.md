---
name: manage-automations
description: Create, configure, inspect, pause, resume, update and troubleshoot recurring agent tasks from natural-language instructions.
---

# Manage automations

Use `manage_automations` for list, create, update, enable, disable, complete and delete operations. Changes to this chat's own automations need no approval; the user stops them from Workspace Tasks. Automations are managed separately from Integrations but share the normal runtime's skills, tools, hooks and permissions.

Interpret the user's instructions, repeat pattern and time. Default to continuing the current chat and its model when available. Use an IANA timezone; ask only when the intended timezone or timing cannot be determined. Calendar schedules contain repeat daily/weekdays/weekly, time HH:MM, timezone and weekday 0=Monday through 6=Sunday. Pick exactly one timing: `interval_mins` 2 to 60 for "every N minutes", or a calendar repeat (use `weekdays` for Monday to Friday); never both. Preserve old interval schedules unless the user asks to change them.

To watch something, create an interval automation with `stop_when` (a short goal), an optional `check_command` argv such as `["cat","status.txt"]` (it runs without the model and wakes it only on change; being a shell command it needs the user's approval of that exact argv in Custom mode, and if declined, create the watch without it), and optionally `expires_in_mins` and `max_runs`. Watches expire after 2 hours and 60 runs by default. When the goal is met, call `complete`; do not leave a finished watch running.

Read existing task configuration before editing. Preserve fields the user did not request to change. If the current chat or project cannot be inferred, request the needed destination. Save through the backend, then report the stored `schedule_summary`, `expires_at` and `next_run_at` from the result, not what you intended. Never bypass unattended approvals. Report missing dependencies and scheduling errors; do not claim a future run succeeded. Remove only at the user's request.
