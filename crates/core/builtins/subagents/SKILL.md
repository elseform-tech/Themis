---
name: subagents
description: List the subagents you can start, their models, write access and limits, or hand a task to subagents; also /subagents.
---

# Subagents

The person picked /subagents, so their message starts with "Subagents". A subagent works on its own brief, beside you, and reports back to you. You start them with `spawn_subagents`.

## With no task: list what is available

When nothing follows "Subagents" in the message, change nothing and list:

- **The built-in subagent:** the plain one, with no profile. It reads, lists and searches files and runs shell commands; it may create or change only the `write_paths` you give it, and with none it only reads.
- **Plugins' profiles:** `list_plugins` with kind `subagent`. Each item's `spec.subagents` holds its profiles with `name`, `description`, `model` (none means the subagent default) and `tools`; `read_only` true means it can never change files, otherwise it may change only the `write_paths` you give it. Leave out plugins that are not `enabled`.
- **Limits and models:** run `themis call get_subagent_settings` in the shell. Report from `settings`: whether subagents are on (`enabled`), how many run at once (`max_parallel`), model requests each (`max_turns`), the default model (`model`, or the chat's model when absent), the other allowed models (`models`), and which of `allow_writes`, `allow_mcp`, `allow_web` and `allow_nested` are off. These are set in Settings › Subagents.

Keep it short: one line per subagent. Always finish with one line that starts "Try:" and gives one example request that fits this project, such as `Try: /subagents review the tests in one subagent and the docs in another`.

## With a task: hand it to subagents

When a task follows "Subagents", hand that task to subagents with `spawn_subagents` rather than doing it yourself:

- Split it into independent parts, one subagent each, within `max_parallel`. A part that cannot be split goes to one subagent.
- Use a profile when a part fits its description, otherwise the built-in subagent. Use a model the person named, if it is one of the allowed models.
- Give `write_paths` only to a part that must change files, each its own paths; the rest only read.
- Write each brief so it stands alone: what to look at, what to do, and what to report.

While they work, carry on with anything that does not overlap. When they report, check what they claim before relying on it, then tell the person what each found or changed. If subagents are off, say so and that Settings › Subagents turns them on.