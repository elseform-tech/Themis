# Prompts

Every model-facing prompt Themis writes (the system message, goal notes,
summary and checkpoint requests, runtime nudges, tool-result notes, attachment
headings, eval probes) lives in its own Markdown file under
`crates/core/builtins/prompts/`. Edit the file to change the text the model
receives; no Rust change is needed.

## Where prompts live

Files are grouped by prefix:

| Prefix | Used for |
|---|---|
| `system-` | Pieces of the system message of every ordinary run |
| `goal-` | Goal mode: the continuation note every goal run carries, the "objective updated" note, a message's goal context and goal-turn notes |
| `summary-` and `compaction-checkpoint.md` | Context checkpoints, document summaries, coverage lines |
| `runtime-` | Runtime notes and nudges sent during a run |
| `tool-` | Notes added to tool results |
| `desktop-` | Text the desktop app adds (run workspace, MCP status, attachments, watches) |
| `visions-` | A Vision's run: its system head (who it is, its channel, how it speaks to the person: everyday words with no ids, commands, paths or tool names, the `themis` calls that drive chats, which it never repeats to the person, the autonomy table), the read-only note, the wake message listing what woke it, the note a refused step carries back, and what its commands say when no sandbox can hold the run; `visions-admin.md` is the admin Vision's head in `# hq` in place of `visions-system.md` (the plain voice, the `themis` calls that propose and set up channels and Visions, one proposal per message, set up only on a clear yes and ask again when unclear; `agent-cli-admin-no-reply.md` is the server's answer when it tries to set one up before the person replied) |
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

The built-in skills (`crates/core/builtins/<name>/SKILL.md`) are not prompt
files: the agent loads them on demand. The integration guides among them
(`plugins`, `skills`, `mcp`, `hooks`) carry `{{global_plugins}}`,
`{{project_plugins}}`, `{{project_root}}` and, in `skills`,
`{{personal_skill_folders}}` and `{{project_skill_folders}}`, which the plugin
store fills with its own folders when it lists them
(`PluginStore::guide_paths`).

Text that is not a prompt stays in code: short UI strings, log messages,
human-facing errors, tool JSON schemas and tool descriptions, and the
checkpoint envelopes and ledger markers the runtime parses back out of saved
chats.
