---
name: hooks
description: List, create, change, delete, disable, test and troubleshoot hooks, the commands that run at set points of a run.
---

# Hooks

A hook is a command Themis runs at a set point of a run. The events, in run order, are PromptSubmit, RunStart, BeforeTool, AfterTool, BeforeCompaction and RunEnd. A chat's session also has SessionStart, once when it opens, and SessionEnd, once when the chat is deleted, the app quits or the chat sits idle. `themis hook --help` lists every event and field. Users configure handlers for these events; they do not create new events.

Hooks belong to plugins and are managed with `themis hook …` in your shell. When the request does not say what to do, list the hooks, say what each one does and whether it is on, and change nothing.

Where they live: global plugins in `{{global_plugins}}`, this project's in `{{project_plugins}}`. Read them through `themis`; never edit those files by hand (Themis refuses it). Commands default to global scope; add `--scope local --project {{project_root}}` for the project's own.

## Find

- `themis hook list` lists every hook, global and in this project, with its plugin; `themis plugin list` shows the plugins and whether each is on.
- `themis hook list PLUGIN` prints that plugin's hook definitions in full: name, event, command, matcher, and `enabled`.

## Change

1. Write the definition as one JSON object: `event` and `command` (or `prompt`), optionally `matcher`, `timeout_seconds` (1 to 600), `blocking`. Start from `themis hook list PLUGIN` when you change an existing hook, and keep the fields you do not mean to change.
2. Apply it, inline in quotes, from stdin with `-`, or as a file: `themis hook add PLUGIN NAME '{"event": "BeforeTool", "command": "…"}'` for a new hook, `themis hook update PLUGIN NAME '<json>'` for an existing one. Use `personal` as PLUGIN for a new hook of the person's own; a missing plugin is created. The definition is checked first and every problem is reported at once with the exact shape; fix them all and run it again.
3. Switch one off with `themis hook disable PLUGIN NAME`; remove one with `themis hook delete PLUGIN NAME` (only when asked).

A new or changed hook is saved switched off. Switching a hook on is the person's alone: Themis refuses it from you in every mode. Tell the person which hook to switch on in Integrations.

## Verify

- `themis hook list PLUGIN` again: the store's own answer is what you report, not what you meant to save.
- `themis hook test PLUGIN NAME` runs the command once with a sample event, inside your sandbox; do it only when the person asks, with safe inputs. A failure may be the sandbox: then ask the person to run the same test in their own terminal.
- `themis doctor --integrations --no-start` after every change: it lists each problem with where it is and a one-line fix.

## How a hook runs

A command receives JSON on stdin with `event`, `run_id`, `thread_id`, `project_root`, and `tool` and `arguments` on tool events (`prompt` on PromptSubmit). It may print JSON. To stop the action it prints `{"decision":"block","reason":"..."}`; BeforeTool, PromptSubmit, RunStart and BeforeCompaction hooks can block, and the reason is shown. After events cannot undo what already happened. A PromptSubmit hook's output is context for the model, labelled as coming from the hook. `matcher` is a `|`-separated list of exact tool names, such as `shell|background_task`, and only applies to tool events. Set `blocking` when a hook that fails (rather than one that asks to block) must also stop the action.

Hooks run outside the sandbox with the person's permissions. Hooks never grant permissions, and enabling a hook does not approve it. Never approve a hook on the person's behalf. Keep commands short, avoid recursive invocation, and report any imported event that was skipped.