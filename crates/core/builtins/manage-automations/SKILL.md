---
name: manage-automations
description: Create, configure, inspect, pause, resume, update and troubleshoot recurring agent tasks from natural-language instructions.
---

# Manage automations

Use `manage_automations` for list, create, update, enable, disable and delete operations. Automations are managed separately from Integrations but share the normal runtime's skills, tools, hooks and permissions.

Interpret the user's instructions, repeat pattern and time. Default to continuing the current chat and its model when available. Use an IANA timezone; ask only when the intended timezone or timing cannot be determined. Calendar schedules contain repeat daily/weekdays/weekly, time HH:MM, timezone and weekday 0=Monday through 6=Sunday. Preserve old interval schedules unless the user asks to change them.

Read existing task configuration before editing. Preserve fields the user did not request to change. If the current chat or project cannot be inferred, request the needed destination. Save through the backend and inspect the resulting next run. Never bypass unattended approvals. Report missing dependencies and scheduling errors; do not claim a future run succeeded. Remove only at the user's request.
