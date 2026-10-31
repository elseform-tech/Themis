---
name: hooks
description: List, create, change, delete, enable, disable, test and troubleshoot hooks, the commands that run at set points of a run.
---

# Hooks

A hook is a command Themis runs at a set point of a run. The events, in run order, are PromptSubmit, RunStart, BeforeTool, AfterTool, BeforeCompaction and RunEnd. Users configure handlers for these events; they do not create new events.

When the request does not say what to do, list the hooks, say what each one does and whether it is on, and change nothing.

- **List:** `list_plugins` with kind `hook`. Each plugin's `spec.hooks` holds its hooks, and `revision` is its `expectedRevision`. This is all you need to read them: do not search files or ask for the marketplace catalog.
- **Create or change:** `manage_integrations` with action `save_hook`, `name` (the plugin; use `personal` for a new hook of the user's own), `hook` (`name`, `event`, `command`, and optionally `matcher`, `timeout_seconds` from 1 to 60, `blocking`) and `expectedRevision` when the plugin already exists. Do not use `save` to change one hook: `list_plugins` leaves out each plugin's files, so saving what it returned would lose them. A new or changed hook is saved turned off.
- **Enable or disable:** `set_component_enabled` with the plugin as `name`, kind `hook`, the hook name as `id`, and `enabled`. Enable only when the user asks. Review a changed command before enabling it.
- **Delete:** `remove_component` with the plugin as `name`, kind `hook`, the hook name as `id`.
- **Test:** `test_hook` with the hook's configuration, only when the user asks: it runs the command. Test with safe resources.

Use global scope for new hooks so they apply in every chat, and keep the scope of existing ones.

A command receives JSON on stdin with `event`, `run_id`, `thread_id`, `project_root`, and `tool` and `arguments` on tool events (`prompt` on PromptSubmit). It may print JSON. To stop the action it prints `{"decision":"block","reason":"..."}`; BeforeTool, PromptSubmit, RunStart and BeforeCompaction hooks can block, and the reason is shown. After events cannot undo what already happened. A PromptSubmit hook's output is context for the model, labelled as coming from the hook. `matcher` is a `|`-separated list of exact tool names, such as `shell|background_task`, and only applies to tool events. Set `blocking` when a hook that fails (rather than one that asks to block) must also stop the action.

Hooks run outside the sandbox with the user's permissions, and execution follows the active YOLO/Custom policy. Hooks never grant permissions, and enabling a hook does not approve it. Never approve a hook on the user's behalf. Keep commands short, avoid recursive invocation, and report any imported event that was skipped.
