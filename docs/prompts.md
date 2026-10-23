# Prompts

Every model-facing prompt Themis writes (the system message, goal checks,
summary and checkpoint requests, runtime nudges, tool-result notes, attachment
headings, eval probes) lives in its own Markdown file under
`crates/core/builtins/prompts/`. Edit the file to change the text the model
receives; no Rust change is needed.

## Where prompts live

Files are grouped by prefix:

| Prefix | Used for |
|---|---|
| `system-` | Pieces of the system message of every ordinary run |
| `goal-` | Goal mode: the goal check, the goal context and goal-turn messages |
| `summary-` and `compaction-checkpoint.md` | Context checkpoints, document summaries, coverage lines |
| `runtime-` | Runtime notes and nudges sent during a run |
| `tool-` | Notes added to tool results |
| `desktop-` | Text the desktop app adds (run workspace, MCP status, attachments, watches) |
| `eval-` | Evaluation harness probes |

`crates/core/src/prompts.rs` exposes each file as a constant
(`themis_core::prompts::SYSTEM_INSTRUCTIONS` and so on) with a one-line comment
saying where it is used. Code that builds a prompt from several pieces keeps
the assembly (separators, optional parts, labels chosen at run time) and takes
only the text from these files.

## Format

- The file's bytes are the bytes sent. Files have **no trailing newline**; a
  test fails if one does, so turn off "insert final newline" for this folder
  in your editor or strip it before committing.
- Line breaks and blank lines inside a file are sent as written.
- Placeholders are `{{snake_case}}` names, filled at run time by
  `prompts::fill`. Keep every placeholder a file already has; do not add new
  ones without changing the code that fills it. In debug builds `fill` panics
  when a placeholder is left unfilled or a value has no placeholder, and the
  inventory test checks every file fills completely.
- A few files begin with words the runtime or clients recognize in saved chats
  (for example `Suspended run progress (`, `Prepared input coverage:`,
  `Document semantic capsule`, `[Runtime check, not from the user]`). Keep
  those opening words; a test fails if they change.

## Applying changes

Prompts are compiled in with `include_str!`. Rebuild (`cargo build`, or restart
`npx tauri dev`) for an edit to take effect; a running app keeps the text it
was built with.

`crates/core/src/prompts/tests.rs` lists every prompt and pins the moved text
to the original wording. After an intentional edit, update the matching
expected text in that file in the same change.

Summary stage caches fingerprint the full request, so editing a summary prompt
produces fresh stages rather than reusing old ones.

## Configurable summary guidance

Without rebuilding, users can add guidance to context and document summaries
with `compaction.prompt_file` in `runtime.jsonc` or a project's
`.themis/config.jsonc` (see [configuration](configuration.md)). It is UTF-8
Markdown of at most 32 KiB, resolved within the directory of the configuration
that declares it. Its text is appended after the built-in summary prompt under
the heading in `summary-configured-guidance.md`; it supplements the built-in
rules and does not replace them.

Text that is not a prompt stays in code: short UI strings, log messages,
human-facing errors, tool JSON schemas and tool descriptions, and the
checkpoint envelopes and ledger markers the runtime parses back out of saved
chats.
