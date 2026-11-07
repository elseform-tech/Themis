# Configuration, scope, permissions, and diagnostics

Desktop and CLI use the same local app server. The one the desktop app starts quits with the
app (it watches its end of a pipe from the app, so a crash or force quit ends it too); one the
CLI starts runs until `themis server stop`. Any server stops by itself once its data folder's
`server.json` has not named it on two checks two seconds apart (the folder or the file was
deleted, so no client can find it), logging `server`/`discovery_lost` when the folder still
exists. Runtime configuration is an
advanced file/CLI feature; the desktop does not expose raw configuration or logs.
The shared API returns effective values, field provenance, schema, and invalid
configuration for repair. Runs reject invalid configuration rather than ignoring it.

## Files and scope

The production macOS app-data directory is
`~/Library/Application Support/ai.themis.desktop/`. Linux uses
`${XDG_DATA_HOME:-~/.local/share}/ai.themis.desktop/`; Windows uses
`%APPDATA%/ai.themis.desktop/`. CLI `--data-dir DIR` selects an isolated store.
QA builds with a different app identifier have separate data directories.

| Location | Purpose | Scope |
| --- | --- | --- |
| App-data `runtime.jsonc` | Runtime defaults, Custom approval rules, OS sandbox, logging, discovery. A new file is short: common settings grouped by purpose, each one whole line commented out under a short note (`// "…` lines are settings, other comments are notes), so deleting `//` turns one on. Every option, its allowed values and default are in `runtime.reference.jsonc` beside it (from `crates/core/src/configuration_reference.jsonc`, which a test keeps complete and equal to the defaults); Themis rewrites that file when it differs and never reads it. Settings › Browser and Web add their blocks without removing the comments. A file with any setting in it is never rewritten; an older starter that sets nothing is replaced by the current one and kept once as `runtime.previous.jsonc` | User, across projects |
| Project `.themis/config.jsonc` | Project runtime overrides and additional approval and sandbox restrictions | Selected project/run root |
| App-data `settings.json` | Existing desktop preferences and legacy run defaults | User |
| App-data `compatibility-cache/` | Derived marketplace scan progress and validated package snapshots | User; refreshed explicitly |
| App-data `plugins/registry.json` | Revisioned plugin, skill, MCP, and hook definitions and activation | Global |
| Project `.themis/plugins/` | Project integration store | Project |
| `~/.agents/skills`, `~/.codex/skills`, `~/.claude/skills` | Discovered original user skills | Global, read-only discovery |
| Project `.agents/skills`, `.codex/skills`, `.claude/skills` | Default discovered project skills | Project, read-only discovery |
| App-data `logs/diagnostics-0.jsonl` | Active structured diagnostic records | User/server |
| App-data `logs/diagnostics-1.jsonl`, `diagnostics-2.jsonl` | Rotated records | User/server |
| App-data `sessions.sqlite3` | Conversation history, run events, compaction checkpoints | User/server |
| App-data `usage.sqlite3` | Token usage: one row per completed model request (35 days), then daily totals (400 days); numbers and ids only | User/server |
| App-data `browser/profiles/<project key>/` | The built-in browser's profile (cookies, logins, cache) for one project's chats; never your own browser profile | Project, in app data |

JSONC is configuration; JSONL is the diagnostic event stream. There is no
separate approval-policy JSONL file. Approval decisions are diagnostic records;
approval rules live in runtime JSONC. API keys stay in Keychain or the launch
environment, not these configuration files.

## History paging

`get_thread_history_page` accepts `threadId`, optional `cursor` (default
`{"after_seq":0,"offset":0}`), optional `throughSeq`, and optional `maxBytes`
(default and maximum 1 MiB; minimum 16 bytes). These are read-request options,
not runtime JSONC or integration definitions. Each page contains at most 128
completed records, bounded UTF-8 `jsonl`, a `through_seq` snapshot watermark and
an optional `next_cursor`. `after_seq` names the last completely delivered
SQLite row; `offset` counts serialized UTF-8 bytes within the next row for this
chat. Foreign-chat rows are excluded even when sequence numbers have gaps.

Pass the returned watermark as `throughSeq` and cursor as `cursor` until
`next_cursor` is null. Chunks can end within a JSON record but never within a
UTF-8 code point. Preserve the unfinished line between calls; parse complete
lines as history items. A cursor/watermark can resume after server restart while
the chat still exists. Rows and originals are never rewritten or truncated.
Start a fresh snapshot after the prior watermark to retrieve subsequent appends.

SQLite body reads and RPC frames are bounded, including a single oversized
history entry. Shared desktop/CLI clients transparently reassemble pages for
the existing `get_thread_history` API, so full-history client memory is not
bounded. Direct legacy RPC responses above 8 MiB return an actionable error
instead of EOF; diagnostics record only the method and size-limit failure.

Credential reads run outside the async backend workers and wait at most 30
seconds, so a blocked read does not freeze other status/Stop requests. Stop
cancels a run's credential wait before skill/MCP setup. The native OS read may
finish later; no authentication requirement is bypassed. Credential status reports
a confirmed missing key as false; OS access/decoding failures, a lookup timeout or worker failure return an
error instead of overwriting the last confirmed UI status. The credential value
never reaches the UI. Presence refreshes on app focus/visibility return, backend
reconnection and run completion; concurrent refreshes coalesce. Run admission is
visible before credential lookup or document preparation, with Stop available
throughout. A running chat hides the connection hint even if the previous presence
check was unavailable; sending still performs the normal credential lookup.

OS lookup errors are preserved through the shared server and CLI rather than
reported as a missing credential. Only a missing-entry result means absence.
macOS errors expose a numeric Keychain status and fixed guidance; other platforms
use fixed error text, without credential bytes or raw credential-store errors.
The OpenCode Go key is the Settings › Secrets secret named by the app setting
`go_secret` (default `OPENCODE_KEY`; letters, digits and `_`, up to 128; set in Settings ›
Models › Providers), stored as the credential entry `key:<name>`. Its value is the saved
secret, else the environment variable of that name. For `OPENCODE_KEY`, an entry saved
under the old name `go` is copied once when nothing is saved yet, never read again
after, and cleared when the secret is removed. A valid stored key keeps precedence. No authentication is
bypassed and no credential value is returned to the UI or CLI status command.

Integrations → Discover installs into **User (Global)** scope without a destination
selector. Project-scoped imports remain available through the advanced CLI/shared API.
Global packages are available across projects; local packages retain their own
scope identity. Use shared import/edit/export APIs rather than hand-editing the
revisioned registry. Portable plugin manifests and MCP JSON are import sources,
not a second configuration store. See [plugins](plugins.md) for formats.
An MCP import's custom file path selects the source to read once. Imported
global servers share `plugins/registry.json`; each server does not get a separate
active configuration file. Project imports use the project integration store.

## Runtime resolution

Resolution order is built-in defaults → user → project → explicit run override.
Objects merge by field; arrays replace previous arrays. Unknown fields,
unsupported schema versions, invalid types, out-of-range values, and a key set
twice in one object (JSON would silently keep only the last) fail, naming the line.
Configuration files are limited to 1 MiB. JSONC accepts `//` and `/* */` comments
and trailing commas. `$schema` is optional editor metadata; `schema_version` is 1.

Runtime values, integration revisions, and thread approval mode are captured at
the start of a run. Changes apply to the next run. Threads currently share the selected project checkout. Project configuration
and guidance resolve from that checkout; user configuration applies to every
run. A manually selected worktree is treated as its own project root.

Every ordinary model run includes that captured project root in its system
context, including after compaction and on short follow-ups. Paths mentioned in
skill descriptions, attachments, examples, or older tasks do not redefine the
current workspace. This context metadata does not change tool approval rules.

Optional `context_token_budget` and `total_turns` override Settings › Advanced
(Working memory, Steps per reply); omit them to keep those settings.
`recent_messages` is file-only (default 20).

### Context budget

`context_token_budget` is `"auto"` or a number. Auto (the default, and what an
omitted field means when Settings › Advanced › Working memory is also Auto)
uses the selected model's context window, up to an Auto ceiling of 200,000
tokens (`AUTO_CONTEXT_CEILING` in `crates/core/src/context_window.rs`, the one
place it is set). A number is a cap: the run uses the smaller of the cap and the
model's window, so a cap above the Auto ceiling lets a model with a larger window
use it. Settings › Advanced offers caps of 200,000–1,000,000 only, so each model's
safe minimum is the smaller of 200,000 and its window; `runtime.jsonc` still accepts
2,000–1,000,000, but budgets under 200,000 are impractical for real work and are for
evals. A run whose checkpoints keep coming every request or two (see
`compaction.thrash_checkpoints`) stops and says the smallest budget that would leave
room to work. Layers override each other as usual, so a project
`"auto"` undoes a user cap.

The window comes from the public model catalog. When the catalog does not list
the model, runs start from a 128,000-token guess. When a request fails with a
context-length error, the run learns that model's limit (the number the error
states, or 90% of the failed request when it states none, never below 8,000),
shrinks its budget to it, compacts once and retries. Learned limits last for
the app session and apply to later runs of that model; they only ever lower
the window. Diagnostics label the window's `capacity_source` as `catalog`,
`learned`, `guess`, or `caller_budget_unverified` (tests and override
endpoints, where the configured budget is used as is).

A goal whose first turn starts with an effective window below 128,000 tokens
gets one note in the chat: "Context window is small (N tokens); long goals may
stall."

Settings files from before Auto stored `context_token_budget` 200,000 by
default. On load, that value becomes Auto (the same budget at the current
ceiling); any other stored number of 200,000 or more stays a cap, and a smaller one
(or a `context_token_cap` saved below 200,000) becomes Auto. The app setting is now
`context_token_cap` (`0` is Auto). The retired `max_turns` and
`context_messages` settings are ignored when old files are read.

Example user `runtime.jsonc`:

```jsonc
{
  "schema_version": 1,
  "context_token_budget": "auto",
  "logging": { "level": "info" },
  "approval": {
    "default": "ask",
    "rules": [
      { "tool": "*", "risk": "read", "action": "allow" }
    ]
  },
  "sandbox": {
    "enabled": true,
    "backend": "auto",
    "network": "allow",
    "writable_paths": ["~/.cargo"]
  },
  "compaction": {
    "timeout_seconds": 900,
    "document_batch_timeout_seconds": 3600,
    "rate_limit_backoff_seconds": 10,
    "retry_limit": 2,
    "summary_max_tokens": 8192
  }
}
```

Example project `.themis/config.jsonc`:

```jsonc
{
  "schema_version": 1,
  "instructions": ["AGENTS.md"],
  "skills": { "paths": [".agents/skills", "team-skills"] },
  "approval": {
    "rules": [{ "tool": "git", "risk": "write", "action": "deny" }]
  },
  "sandbox": { "network": "block" }
}
```

The `sandbox` block configures the OS sandbox for commands the agent runs
(`shell`, background tasks, the `git` tool and watch checks) in Custom; YOLO runs
use an open plan that keeps the app data and `.themis/` read-only and keeps
`network: "block"` and `allow_keychain: false` only when a file or override wrote
them (their provenance is not `default`); `writable_paths` does not matter there. See [sandbox](sandbox.md) for what it allows, the secret locations it
hides, backend choice and limits. A project file and an explicit run override may
only tighten it (block the network, enable it, drop writable paths); anything
else fails validation with a message naming the field.

`skills.paths` replaces project skill search roots; it does not change conventional
user discovery. Skill and instruction paths must be relative, without parent
traversal, and remain inside the project after canonicalization. Each list permits
at most 32 paths. Instructions are UTF-8 project guidance, limited to 64 KiB
combined, and do not grant permissions.

`project_guidance` (default `true`) reads the project root's `AGENTS.md`, or `CLAUDE.md`
when there is no `AGENTS.md`, at the start of each run and puts it in the system message of the
agent and its subagents. They also get a map of up to 40
other documents (`.md`, `.mdx`, `.rst`), nearest the root first, three folders deep at most, with
hidden and generated folders skipped and links not followed; these are named, not loaded, and a
guidance file in a subfolder applies to work in that folder. A file over the 64 KiB limit is cut;
one that can't be read is left out without failing the run. Set it to `false` to keep only the files
listed in `instructions`, which always load, reach subagents as well, and must exist. All of this is
project content, so a cloned repository's guidance reaches the agent, but it never grants permissions.

## Supported limits

| Field | Range/default |
| --- | --- |
| `context_token_budget` | `"auto"` (the model's window, up to 200,000) or a cap of 2,000–1,000,000 clamped to the window; omitted uses Settings (default Auto) |
| `recent_messages` | 2–200; default 20 (file-only) |
| `total_turns` | 1–2,000; omitted uses Settings |
| `logging.level` | `debug`, `info` (default), `warn`, `error` |
| `compaction.timeout_seconds` | 1–1800; default 900 per context checkpoint or uncached document job, including stages, retries and consolidation; explicit existing values are preserved |
| `compaction.document_batch_timeout_seconds` | 1–21,600; default 3,600 for the complete attachment batch, including source persistence, cache lookup, all documents and publication. A smaller batch limit shortens document jobs. |
| `compaction.rate_limit_backoff_seconds` | 1–300; default 10 for the first summary HTTP 429 retry, then twice that base; jitter adds up to half the base. Other transient errors retain one-/two-second bases. Ordinary answer backoff is unchanged. |
| `logging.context_metrics` | `off`, `summary` (default), `detailed`; controls content-free accounting, never safety guards |
| `compaction.request_timeout_seconds` | 1–300; default 300; additionally limited by the remaining job/stage allocation |
| `compaction.maintenance_threshold_ratio` | 0.50–0.95; default 0.83 of usable message allowance |
| `compaction.semantic_threshold_ratio` | Above maintenance and at most 0.97; default 0.93 |
| `compaction.target_headroom_interactions` | 1–20; default 6; recent observed growth adjusts the trigger within a bounded reserve |
| `compaction.thrash_checkpoints` | 0–50; default 6. A run stops when this many checkpoints in a row each came within two model requests of the one before: its budget is too small to make progress. The error says how much room the instructions and tools leave and the smallest budget that leaves 6,000 tokens for work; `0` never stops. Separately, a budget that leaves under 1,000 tokens after the instructions and tools stops the run before its first request, saying how much they need and the smallest budget to set (`context` / `budget_below_instructions` in `logs/`) |
| `compaction.max_stages` | 1–128; default 32, including consolidation. Applies to attached-document preparation; context checkpoints always use one request |
| `compaction.retry_limit` | 0–2; default 2 (up to three attempts per failed stage) |
| `compaction.document_context_token_budget` | Internal full-document preparation input budget, 24,000–1,000,000; default 200,000; clamped to the routed summary model capacity, with conservative 24,000 when unknown. Does not change ordinary context capacity. |
| `compaction.summary_max_tokens` | Initial summary-output ceiling, 256–16,384; default 8,192. Retries may use up to twice the effective initial allowance, subject to capacity. |
| `compaction.summary_model` | Optional model id (1–256 printable bytes, no spaces): any model a connected provider offers, `<provider>/<id>` for one from `providers`; omitted/null keeps automatic routing. Only internal summaries change. A run fails before it starts when the model is not in the captured catalogs (OpenCode Go's and the configured providers'); a model on another provider than the chat's is called with that provider's key. |
| `hooks.max_wakes` | Integer 0–10, default `3`: how many turns in a row a finished background [plugin hook](plugins.md) may start in an idle chat before you write again. `0` never starts one, and what a hook reports waits for the chat's next turn. Never in YOLO, while a goal runs, or after a turn that failed or that you stopped. A project or run layer may only lower it. |
| `hooks.session_idle_minutes` | Integer 0–10080, default `60`: minutes a chat may sit idle before its session ends for [plugin hooks](plugins.md#sessions) (`SessionEnd`, then `SessionStart` when you write again). `0` never ends one for being idle. Captured when a run starts, so a change applies from the chat's next turn. |
| `hooks.prompt_hook_calls` | Integer 0–1000, default `50`: model requests [prompt hooks](plugins.md#prompt-hooks) may make in a run, shared with its subagents. A guard that finds none left refuses the call. `0` turns prompt hooks off. Your own `runtime.jsonc` only: a project or run layer that sets it is refused. |
| `memory.enabled` | Boolean, default `true`: as the person speaks the memory models keep memories across chats in the background (see [Memory](#memory)), and every run (and subagent) starts with the memory index and the memories relevant to its message. No agent has a memory tool. `false`: nothing is saved or read and no memory job runs; saved memories are kept, and the main agent's system message carries the one line in `crates/core/builtins/prompts/system-memory-off.md` instead, so it says it can't remember rather than promising to. Settings › Memory's switch writes it in your `runtime.jsonc` in place. A project or run layer may only set it to `false`. |
| `memory.models` | 3 to 8 distinct model ids (`<provider>/<id>` for a model from `providers`), default `["muse-spark-1.3-contributor", "claude-haiku-5-5", "deepseek-v4.1-flash"]`: the models that run memory jobs, tried in order. A job moves to the next model on a provider error, a time-out (`compaction.request_timeout_seconds`), a rate limit (HTTP 429), an answer it can't read, or the daily cap. Settings › Models › Memory models edits the list in place (reorder, add, remove; never fewer than three). The defaults are cheap OpenCode Go models on three different wire protocols (Responses, Messages, Chat Completions), each with structured-output support in the models.dev catalog. Your own `runtime.jsonc` only: a project or run layer that sets it is refused. |
| `memory.daily_budget_usd` | Number 0–100, default `0.5`: estimated dollars (models.dev list prices, as in Settings › Usage) all memory models may spend in a day together, counted from memory usage recorded since local midnight. A model the catalog lists as free is never held back; a model without a price counts as paid at $0. When every model is held back, the job waits until tomorrow without using up its tries. File only; your own `runtime.jsonc` only. |
| `memory.consolidate_every_hours` | Integer 1–720, default `24`: at most one idle consolidation (merge duplicate memories, drop clearly stale ones) this often, recorded in app data `memory/consolidated.json`. File only; your own `runtime.jsonc` only. |
| `subagents.enabled` | Boolean, default `true`: offer the subagent tools (`spawn_subagents`, `wait_subagents`, `message_subagent`, `stop_subagent`) to the agent. A project or run layer may only set it to `false`. |
| `subagents.max_parallel` | Integer 1–6, default `3`: subagents running at once per chat; more wait their turn. A project or run layer may only lower it. |
| `subagents.max_turns` | Integer 1–1000, default `40`: model requests per subagent; reaching it ends the subagent with its partial result. A project or run layer may only lower it. |
| `subagents.model` | Optional model id, as `compaction.summary_model`: the model subagents use; omitted/null uses the chat's model. Any layer. |
| `subagents.models` | Up to 3 model ids, as `subagents.model`, default `[]`: further models the agent may give one subagent, for example when you name one ("use glm-5 for the review"). Any other model is refused. A subagent whose model cannot start, or whose first request fails, falls back to `subagents.model`, then the chat's model. A plugin's [subagent profile](plugins.md#subagent-profiles) with a `model` uses the entry it matches, else `subagents.model`. A project or run layer may only narrow the list. |
| `subagents.allow_mcp` | Boolean, default `true`: `false` gives no subagent MCP tools, whatever a profile names or the agent asks for. A project or run layer may only set it to `false`. Otherwise a subagent gets MCP tools only from the connections you enabled that came up when the run started: those its profile's list allows, or those the main agent lists in `mcp_tools`. They can act outside the project, and a subagent's read-only file limit and write paths don't cover them. Every call goes through the chat's approval mode under the subagent's name, deny rules still win in YOLO, its "Allow in this chat" stays its own, and `BeforeTool` and `AfterTool` hooks fire with `agent`. What it can't have is left out and it is told; `subagents` / `mcp_left_out` logs the reason (`setting` or `not_connected`), the count and at most 20 names. |
| `subagents.allow_web` | Boolean, default `true`: `false` gives no subagent `fetch_url` or `web_search`, whatever a profile names. A project or run layer may only set it to `false`. Otherwise a subagent has them unless its profile's list leaves them out, with the same approval rules, per-site grants, deny rules (YOLO included) and hooks as the main agent's, asking under the subagent's name. `sandbox.network: "block"` also keeps them from subagents. `subagents` / `web_left_out` logs the reason and count when a list names them and they can't be had. |
| `subagents.allow_nested` | Boolean, default `true`: `false` stops subagents starting subagents, whatever a profile names. A project or run layer may only set it to `false`. Otherwise a subagent can start its own only when its plugin profile's list names `Agent` or `Task`; see [plugins](plugins.md). Those can't start any themselves. |
| `subagents.allow_writes` | Boolean, default `true`: `false` keeps every subagent read-only whatever paths the agent gives it. A project or run layer may only set it to `false`. The agent must give every subagent `write_paths` (`[]` for one that only reads; a spawn that leaves it out is refused before anything starts, so a builder is never started read-only by mistake). Otherwise a subagent given `write_paths` (project-relative folders ending in `/` or files, never globs) may change only those: its file tools are limited there in every mode, and with an enforcing sandbox so are its shell commands, YOLO included. Paths that overlap another running subagent's, leave the project, or reach `.git/`, `.themis/` or the app data are refused. With no enforcing sandbox its shell runs as the main agent's does, and the subagent's detail says "Shell not confined". On Linux, file paths (as opposed to folders) are writable through the file tools only, since the OS sandboxes can't allow a file that does not exist yet. |
| `providers` | User file only; at most 16 OpenAI-compatible providers. See [Model providers](#model-providers-and-per-model-overrides) |
| `models` | At most 256 per-model overrides keyed by model id or `<provider>/<id>`; any layer. See [Model providers](#model-providers-and-per-model-overrides) |
| `compaction.prompt_file` | Optional UTF-8 Markdown, at most 32 KiB |
| `approval.sensitive_files` | File-name patterns (at most 128, each 1..128 bytes, `*` and `?`, no case, no paths) of project files that ask first in Custom when read, written, staged or committed. A later pattern wins and `!` exempts. Default: `.env`, `.env.*`, `!.env.example`, `!.env.sample`, `!.env.template`, `*.pem`, `*.key`, `id_rsa`, `credentials.json`, `.npmrc`, `.pypirc`, `secrets.*`. User and run lists replace it, as arrays do, and `[]` turns it off. A project's list only adds: `[]` there changes nothing and a `!` pattern is a validation error that refuses runs. Rules with `"tool": "sensitive_file"` decide for these files (deny works in YOLO too; `command` is `["read" \| "write" \| "commit", path]`). Best effort for shell commands and no wall: see [sandbox](sandbox.md#sensitive-project-files). `approval` / `sensitive_file_asked` logs the tool, the kinds of file (`env`, `key`, `credentials`, `registry`, `custom`) and a count, never a path; a path can name a client or a machine, and it is a tool payload. `approval` / `secret_scan` logs the time and counts of the check when a run starts. `themis permissions` prints the list. |
| `approval.rules` | At most 256 ordered rules `{tool, action, risk?, command?}`; `command` is an argv-prefix array (see Composer permissions) |
| `sandbox.enabled` | Boolean, default `true`; only the user file (or no file) can turn it off |
| `sandbox.backend` | `auto` (default), `seatbelt` (macOS), `bubblewrap` or `landlock` (Linux); a backend of another OS is an error, an unusable named one fails closed |
| `sandbox.network` | `allow` (default, also in Custom) or `block`; projects may only set `block` |
| `sandbox.writable_paths` | At most 32 folders, absolute or `~/...`, 1–1,024 bytes, no `..`; projects may only remove entries |
| `sandbox.allow_keychain` | macOS: `false` (default) keeps sandboxed commands out of the Keychain; `true` lets them use it (for example git's HTTPS credential helper); projects may only set `false` |
| `web.search_url` | Optional http(s) URL of your SearXNG instance (JSON output enabled), at most 1,024 bytes, no credentials; user file only. Unset: the SearXNG built into Themis is used (when the build has it; otherwise `web_search` is not offered). See [web tools](web-tools.md) |
| `browser.enabled` | Boolean, default `true`; projects may only set `false` |
| `browser.executable` | Optional absolute path to a Chrome, Chromium or Edge binary, at most 1,024 bytes; user file only. Unset: the built-in engine (downloaded on first use), else an installed browser |
| `browser.developer_mode` | Boolean, default `true` ("Enable full CDP access"): off offers no debugging tools on any page; projects may only set `false` |
| `browser.permissions` | `browsing`, `downloads`, `debugging`: each `"allow"`, `"ask"` or `"block"`; defaults allow, allow, block. Projects may only make a value stricter. See [web tools](web-tools.md#permissions-settings--browser) |
| `browser.site_exceptions` | Up to 500 `{ "site": "example.com", "browsing"?, "downloads"?, "debugging"? }`: a lowercase host covering its subdomains, most specific wins. Default: the starter blocks. A project's exceptions may only block, and they add to yours instead of replacing them (an exception to "arrays replace") |
| `browser.show_secrets` | Boolean, default `false`: show cookie values, auth headers and token-like values to the agent; projects may only set `false` |
| `browser.downloads_directory` | Optional absolute folder for downloads; user file only. Unset: the system Downloads folder |
| `browser.open_workspace` | Boolean, default `false`: open Workspace on the Browser tab when the agent starts browsing (otherwise the chat's overview box shows a live preview); user file only |
| `browser.screenshots_to_model` | Boolean, default `true`: when the run's model reads images (`models.<id>.images`, else the models.dev catalog), a `browser_screenshot` it takes also reaches it as an image, at most 1,280 pixels on the longest side and 1.5 MB, in a user message right after the tool result; each request carries only the latest two, older ones go as their text result. The goal check's model gets the turn's latest two the same way when it reads images. `false`, or a model that reads text only, keeps the text result. Projects may only set `false` |

`prompt_file` supplements the built-in compaction guidance; it does not replace
the compaction algorithm. It resolves within the directory of the configuration
that declares it. Explicit run overrides resolve prompt references within the
user configuration directory. Built-in prompts are in
`crates/core/builtins/prompts/` (see [prompts](prompts.md)). The shared adaptive pipeline first performs reversible maintenance of old public tool intent, completed invocation arguments and redundant recovery copies. Delivered ordinary results remain available until semantic summarization consumes them; completion receipts retain pre-offload provenance. Maintenance checks for net savings before publishing a snapshot. It summarizes only under working-context pressure, after accounting for schemas, the actual output allowance and a safety margin. Archive size and reaching a tool-turn milestone do not themselves trigger summarization.

Each context checkpoint is one summary request. When all eligible new records fit, they are supplied whole. Otherwise long fields are cut to an equal head/tail excerpt ceiling with UTF-8 byte coordinates, and when even empty fields cannot fit the oldest new records are left out; the request carries a coverage notice saying so, and cut checkpoints require saved originals. Omitted detail stays in the saved originals, which the `recall` tool searches; the checkpoint is a map (goals, decisions with reasons, unresolved and parked work, next action, pointers), not a full transcript. The result must pass the ordered, nonempty state-section checks before it is installed; this validates structure, not factual correctness. Attached documents still use the staged pipeline: when a complete document cannot fit, its originals are split into ordered, UTF-8-safe source fragments, summarized sequentially and consolidated once, completed stages persist under the evidence directory's `summary-jobs/` and are reused internally for matching source, prompt, model and output policy, and a configured stage-count or packet-size overflow fails before incomplete state is installed. (Benchmark audit 2026-10-15: on the 24k compaction suite the single request kept summary-only retention within noise of the former staged context path at half the requests and about 43% less checkpoint latency; see `docs/verification/simplify-compaction-2026-10-15.md`.)

Each failed summary request or document stage automatically retries up to twice for transient transport/provider errors, request timeout, incomplete/truncated output, empty text or a malformed required checkpoint. Healthy stages use one call. Every attempt must pass structural validation. Permanent provider errors, storage failures and impossible budget/stage plans fail immediately. Output limits are base, twice base, twice base; preparation reserves the maximum before dispatch. Retry-After remains a floor with exponential backoff and jitter for every retry. Summary HTTP 429 responses use `rate_limit_backoff_seconds` (default ten then twenty seconds); other transient summary errors use one then two seconds. Transient errors are timeouts, HTTP 408, 429 and 5xx, and HTTP 499, which OpenCode Go returns when its upstream endpoint is briefly unavailable; ordinary answers retry the same errors. This also covers upstream-unavailable errors returned with status 429; it does not identify their underlying cause or guarantee provider recovery. The absolute job/stage deadlines include requests and waits: three attempts are a maximum, and the final attempt may have less time. Completed stages and tools are never replayed as part of a request retry. The default overall job deadline is fifteen minutes (900 seconds), configurable up to thirty minutes (1800), while each request remains limited to five minutes (300). Each uncached attachment receives its own job deadline, capped by the separate `document_batch_timeout_seconds` deadline (default one hour). A document deadline spans all of its stages and retries; it is never reset per stage. Explicit existing timeout and retry overrides are preserved. No new summarization calls are made after success. Execution-only changes to job/request/batch timeouts, retry count or rate-limit wait do not invalidate completed stages or capsules. Cache identity still binds the resolved prompt, source, routed model, effective input/output budgets and pipeline. Exact pre-split cache identities can be migrated without provider calls when the current settings reconstruct the old key and no custom resolved prompt is active; the runtime does not scan for or guess unknown historical settings. Migration validates original identities and bounded cached stages before publishing a new immutable key.

`summary_max_tokens` caps the initial allowance, not every attempt. Each permitted retry currently uses up to twice that effective initial allowance, including retries after HTTP 429; increasing output does not itself resolve rate limiting. Provider output limits and the reserved input/output capacity can clamp these allowances further. The checkpoint allowance also depends on the usable message budget, system/current-request/recent-message space and continuation reserves. Increasing the ceiling alone may therefore leave small-budget requests unchanged. A larger effective initial allowance can avoid output-truncation retries, but requires room for both the maximum retry output and continued ordinary work. It cannot restore evidence excluded from summary input. Validated bounded original-derived archive excerpts now enter both summary paths independently of optional read caches; see [context limits and recovery](context-and-attachments.md).

Failure or cancellation keeps the working context unchanged, emits no fallback checkpoint and stops the current run. Before summarization, and before the summary's deadline starts, the runtime saves the original evidence snapshot, which is also the job's exact input (no separate pending copy is written for context checkpoints); a slow disk flush therefore cannot make the summary time out with the originals reported unsaved. Failure messages report whether originals were saved; a storage failure does not imply durable recovery. After a noncancelled checkpoint failure with originals saved, a bounded historical progress record is persisted before the terminal failure. It carries selected tool observations and original-source coordinates into a later ordinary request, including after restart, without restoring or executing the pending job. Its byte ceiling is one sixth of usable message tokens, clamped to 1,024–8,192 bytes; action receipts take priority over retrieval excerpts. It is not a summary, proof of tool success or current file state, or a complete action log. Stop and document-preparation failures do not publish this record. Current user instructions retain precedence. Metadata diagnostics report `originals_saved` and record retry reason, attempt, backoff and remaining time without content; `suspended_progress` records only publication and context-replacement flags. There is no user-facing compaction retry command and no automatic recovery-to-compaction continuation loop.

Internal summaries disable reasoning independently of the selected answer effort. Known off controls retain LongCat 2.5 with thinking disabled and GPT 5.6/6 Luna with reasoning effort none. A summary sent to any other Responses model asks for the lowest effort the catalog lists for it (`none`, else `minimal`, else `low`; `none` when the catalog does not describe the model), since some models, such as muse-spark, reject `none`. Other models use `longcat-2.5-preview-free` with thinking disabled for summaries, through the existing Go credentials and session. This routing is independent of the selected ordinary model. When Go no longer lists that summarizer, summaries use the chat's own model; a Chat Completions model then gets `thinking: disabled` only when its catalog entry lists a thinking switch (or the catalog does not describe it), else its lowest listed effort as `reasoning_effort`, else no thinking field at all, since Go rejects a `thinking` field a model does not take (for example `beam-free`). When Go still lists it but refuses the model itself (HTTP 402, 404 or 410, as when its endpoint is unavailable), that one summary request is sent again with the chat's own model at its lowest reasoning effort, and a `summary_model_unavailable` diagnostic records both model names and the status; other errors keep the ordinary summary retries. An explicitly configured `compaction.summary_model` is never replaced. Diagnostics identify the effective model without recording response text.

An explicit `compaction.summary_model` selects one of those verified nonthinking routes without changing ordinary work. It follows normal user/project/run precedence; null clears an earlier override. Unsupported model IDs fail configuration validation, and a model missing from the loaded catalog fails setup before document preparation or provider generation. Routing, capacity, output limits, request telemetry, stage fingerprints and capsule identity use the same resolved model. There is no automatic model switch after provider failure. Changing the model may require new summaries; existing model-specific stages, capsules and original evidence remain available. Suspended document-job keys bind the resolved model and source path so equally sized models or identical documents at different paths cannot overwrite each other's recovery inputs. Ambiguous legacy pending inputs remain preserved, including during capsule migration; they do not cause automatic replay or new model calls.

Ordinary answers retain the selected model and configured reasoning behavior. Effort validation loads the Go catalog on first use after restart, before deciding whether an effort is supported. Catalog-loading failures remain explicit loading errors; Stop can cancel that fetch during run setup.

Thinking is on by default for answers. When no effort is selected, a model whose catalog entry lists a thinking switch gets it turned on (`thinking: enabled` for Chat Completions; for Messages, a budget of half the output cap), and a model with only effort levels gets `medium`, else `high` (Gemini receives it as `generationConfig.thinkingConfig.thinkingLevel`). A selected effort always wins, models not in the loaded catalog keep their own default, and internal summaries keep thinking off (Gemini cannot turn thinking off, so its summaries use `thinkingLevel: low`).

Each Go model's wire protocol and endpoint come from its models.dev entry: the AI SDK package names the protocol (`@ai-sdk/openai-compatible` or none: Chat Completions; `@ai-sdk/openai`: Responses; `@ai-sdk/anthropic`: Messages; `@ai-sdk/google`: Gemini) and `provider.api` the endpoint. An endpoint is used only when it is an `https://opencode.ai/` URL; otherwise Go's documented endpoint for the protocol under `https://opencode.ai/inference/go/` is used. A package Themis does not speak marks the model unavailable: pickers show it disabled with the reason and runs on it are refused before any request. When the catalog cannot be loaded, or does not list a model yet, the model name decides (`muse-`, `gpt-`, `grok-`: Responses; `qwen`, `minimax-`, `claude-`: Messages; `gemini-`: Gemini; otherwise Chat Completions). A test base URL serves every protocol from its one root. If a loaded catalog no longer lists `longcat-2.5-preview-free`, summaries for models without a verified off control use the model itself.

A thinking model's reasoning goes back to it with its own tool calls, so each step builds on the previous one instead of re-deriving it from tool results. A final answer's reasoning goes back with that answer in the chat's later messages, so the model keeps its line of thought from one message to the next; reasoning for intermediate steps of earlier messages is not sent, because those steps are not in later requests. Chat Completions requests carry the reasoning text on that assistant message under the field it came in (`reasoning_content`, or `reasoning` as Kimi and Hunyuan send it), with any `reasoning_details` entries joined by index; Messages requests carry the `thinking` and `redacted_thinking` blocks unchanged (signatures included) before its text and tool uses; Responses requests ask for `reasoning.encrypted_content` (requests are not stored) and carry the `reasoning` items before their function calls; Gemini requests carry the step's `thoughtSignature` on its first function call (or on a final answer's text). Gemini refuses a function call without its signature, so when the signature is no longer held (replay off, or history from before a restart) Google's documented placeholder `skip_thought_signature_validator` is sent instead. Reasoning is held in memory only, keyed by tool-call id (at most 512 call sets), and is never shown, logged, or saved with history, transcripts or evidence. It stops being sent once its calls leave the request (a checkpoint or restart) and is never sent to internal summaries.
Summary diagnostics identify the effective model without logging response text.
Each ordinary answer request allows at most two retries
before public text, with exponential base delays of one then two seconds and
randomized jitter of 0–500 ms then 0–1,000 ms. A longer `Retry-After` is a minimum;
the jitter is added above it. Each ordinary attempt has up to five minutes, including
the handshake, stream and fallback. All attempts and backoffs share a fifteen-minute
overall deadline, which may shorten the final attempt. An attempt timeout may retry
only before public text. Authentication and invalid-request failures do not retry.
An answer with neither nonblank public text nor tool calls is never accepted as
completion. Empty answers may retry within the same attempt/deadline bound when
no public text was emitted; persistent emptiness fails explicitly. Truncated JSON
responses are rejected even when the provider returns JSON for a streaming request.
Completed tools are not replayed. Metadata diagnostics record retry number,
HTTP status and delay, without provider bodies. Stop can cancel either wait. Originals, prior
usable state and deterministic action/read receipts remain separate from the
model summary. This does not add compaction UI or guarantee lossless recall.

### Background shell task limits

Not configurable in `runtime.jsonc`; fixed constants in the core:

| Limit | Value |
| --- | --- |
| running background tasks per chat / per app server | 4 / 16 |
| rolling log per task | last 64 KiB (page size default 8 KiB, max 16 KiB) |
| per-task deadline | default 7,200 s, maximum 86,400 s (`background_timeout_secs`) |
| finished records kept per chat | 20 |
| output wait after a blocking command exits | 1.5 s |

Approval mode and rules are captured once per run like any shell call; the
approval mode change affects subsequent runs, not tasks already started. See
[sandbox.md](sandbox.md#background-shell-tasks) for behavior and lifetime, and
`get_thread_tasks` (`background_tasks`), `get_background_task_log` and
`stop_background_task` for the shared-server API.

## Model providers and per-model overrides

OpenCode Go is built in. `providers` in the **user** `runtime.jsonc` adds
OpenAI-compatible providers (Chat Completions); a project file or run override
that names `providers` fails validation, since each request carries your key.
Arrays replace earlier arrays, so the list is the user file's. Provenance reports
`providers` as `user`.

```jsonc
{
  "providers": [
    { "id": "local", "name": "Local models", "base_url": "http://127.0.0.1:8080/v1",
      "secret": "${LOCAL_KEY}",
      "models": [{ "id": "qwen3-coder", "context_tokens": 131072, "output_tokens": 16384,
                   "price": { "input_per_mtok": 0.2, "cached_input_per_mtok": 0.05,
                              "output_per_mtok": 0.8 } }] }
  ],
  "models": {
    "glm-5.3": { "context_tokens": 128000, "thinking": false },
    "local/qwen3-coder": { "reasoning_effort": "low" }
  }
}
```

| Field | Range/default |
| --- | --- |
| `providers[].id` | 1–32 lowercase letters, digits, `-`, `_`, starting with a letter or digit; unique; `go` and `opencode-go` are reserved. A chat names the provider's model `<id>/<model>` |
| `providers[].name` | 1–64 printable characters, shown in Settings › Models |
| `providers[].base_url` | The API root, at most 1,024 bytes; `https`, or `http` only for `localhost`, `127.0.0.1` or `[::1]`; no credentials, query or fragment. Requests go to `<base_url>/chat/completions` and `<base_url>/models` |
| `providers[].secret` | A Settings › Secrets name written `${NAME}` (letters, digits, `_`, up to 128); its value is the saved secret, else the environment variable of that name. A literal key fails validation |
| `providers[].models` | Optional, at most 512 `{id, context_tokens?, output_tokens?, price?}` with unique ids (1–256 printable bytes, no spaces). Left out, Themis reads `GET <base_url>/models` (OpenAI-shaped `{"data":[{"id":…}]}`, at most 4 MiB; unusable or repeated ids are skipped and at most 512 kept) with the key and keeps the list until Settings › Models checks again |
| `…models[].context_tokens` | 1,000–10,000,000; the model's window. An unknown window (as for models read from `/models`) starts at the 128,000-token guess and is learned from the provider's context-length error (see [Context budget](#context-budget)); give one with this field or `models.<provider>/<id>.context_tokens` |
| `…models[].output_tokens` | 256–1,000,000; caps every reply's `max_tokens` |
| `…models[].price` | `input_per_mtok`, optional `cached_input_per_mtok`, `output_per_mtok`: US dollars per million tokens, 0–10,000. Carried on the model list for usage reporting; no request uses it |
| `models.<id>.context_tokens` | 1,000–10,000,000; caps the model's window (the lower of the two; an unknown window takes the cap). Applied through the model capacity a run captures, so the context budget is at most this |
| `models.<id>.output_tokens` | 256–1,000,000; caps the model's output limit, never raises it |
| `models.<id>.reasoning_effort` | `none`, `minimal`, `low`, `medium`, `high`, `xhigh` or `max`: the effort when the chat chose none. An OpenCode Go model must offer it (a run on that model fails otherwise); a configured provider's model receives it as `reasoning_effort` |
| `models.<id>.thinking` | `true` or `false`, for a model whose catalog lists a thinking switch: `false` sends thinking disabled instead of turning it on. A run on a model without a switch fails |
| `models.<id>.images` | `true` or `false`: whether the model reads images, so a browser screenshot it takes reaches it (see `browser.screenshots_to_model`). Unset follows the models.dev catalog (`image` among the model's input modalities); a model the catalog does not describe, such as a configured provider's, reads text only until this says `true` |

`models` keys may be a bare id (`glm-5.3`, or `qwen3-coder` for every provider
offering it: a bare key applies to that id from every provider) or
provider-qualified (`go/glm-5.3`, `local/qwen3-coder`); for each field the
qualified entry wins. A prefix must name a provider in `providers` (or `go`
for OpenCode Go in a `models` key): `lcoal/qwen3-coder` is refused rather than
read as an OpenCode Go model, and so is a `compaction.summary_model` or `subagents.model`
whose prefix names no provider (a bare id is OpenCode Go). Objects merge by field across layers, so a
project may lower a cap or set an effort for its runs. Only the run's own model
is checked against its catalog.

Configured providers speak Chat Completions only, without OpenCode's session
header or thinking fields. Their models get no automatic summary routing: a
chat on one summarizes with its own model unless `compaction.summary_model`
names another. Settings › Models lists each provider with its secret and status:
Connected (`/models` answered), Key set (models listed in the file, `/models`
unavailable), Key rejected (401/403), Couldn't reach it, or Not set (no value).
Settings › Secrets lists the secret with the provider's name among its users.
Each check logs `providers`/`provider_checked` with the provider id, result,
model count and duration only; keys and provider response bodies are never logged,
and errors from `/models` carry the HTTP status, not the body.

Shared API: `list_model_providers` returns `[{id, name, secret, status, models}]`
(`status`: `works`, `unchecked`, `rejected`, `unavailable`, `missing`);
`list_go_models` returns OpenCode Go's models followed by every configured
provider's (each with `provider`, and `price` when the file gives one). With
configured providers, an OpenCode Go failure leaves Go's models out instead of
failing the call.

## Composer permissions

The shield offers **YOLO**, **Auto** or **Custom** (default), persisted per thread.
It has no configuration editor or advanced settings link.

**Auto** never prompts. Rules decide first, exactly as in Custom (a deny rule refuses, an allow
rule or an "allow in this chat" grant proceeds), and Custom's sandbox applies; where Custom
would ask, a reviewer model decides instead, against the person's own messages in the chat (the
latest 12, at most 6000 characters), the chat's goal and the project's guidance (at most 3000
characters). Text from files, pages or tool output never counts as the person's request. The
reviewer is `approval.auto_model` (user file only, a model id; a project setting it is a
validation error), else the first memory model; it answers allow, `soft` (risky but plausible:
the action is refused and the agent gets the reason and a safer way, `auto-soft-block.md`; the
third soft block of the same command in a run is a hard one) or `hard` (destructive,
unrequested or driven by content: refused, the agent is told to finish the rest and say what was
blocked, `auto-hard-block.md`, and the chat gets a question box, `auto-blocked-question.md`,
with **Do it the safer way** (recommended), **Allow once** and **Skip it**, each saying what happens with the command itself; the reviewer's `safer` names the exact command or steps). "Allow once" lets
exactly that action run once in the chat (kept in memory, not saved). A review that fails or
takes over 45 seconds is a hard block, so nothing unjudged runs. Gates only a person may open
never go to the model: running a command outside the sandbox and touching a sensitive file are
always blocked with the question box, and only the person's **Allow once** for that exact action
lets them run. On a project that isn't a Git
repository Auto stays read-only, as Custom does, without asking the model. Saving a new MCP
connection command from a run is refused in Auto (allow it in Integrations, or use Custom).
Subagents are reviewed the same way. Prompts: `auto-review-system.md`, `auto-review-user.md`.
`logs/` gets `approval` / `auto_reviewed` (tool, risk, verdict, time) and `approval` /
`auto_review` lines, never the command or the person's words. `auto_try_review` is a read-only
dry run of one review for evals (`evals/auto_review`).
Custom checks rules in order: first matching tool/risk wins, otherwise `default`
applies. Tool names match exactly or with a trailing `*` prefix wildcard.
Actions are `allow`, `ask`, or `deny`; risks are `read`, `write`, `execute`,
`network`, and `destructive`. Built-in policy allows reads and asks for other
actions. The existing confirm-reads setting still applies when no explicit
approval configuration is supplied.

A rule may carry `command`, an array of 1–16 non-empty argv tokens (each at most
256 bytes) that scopes it to `shell` calls: it matches when the program and its
arguments begin with exactly those tokens (`["cargo","test"]` matches
`cargo test --lib`, not `cargo build`, `cargo testing` or `/usr/bin/cargo test`).
No globbing applies. `command` is valid only with `"tool": "shell"` or
`"tool": "shell_unsandboxed"` (exact names; the latter is the request to run a
command the sandbox blocked outside it, see [sandbox](sandbox.md#when-the-sandbox-blocks-a-command)).
Only allow/ask rules naming `shell_unsandboxed` exactly apply to that request;
wildcard, prefix and risk-only allow rules and an `allow` default never cover it,
while every matching deny rule (including `*`) refuses it.
Example allow rule: `{ "tool": "shell", "command": ["cargo", "test"], "action": "allow" }`.

For web and browser tools (`web_search`, `fetch_url`, `browser_*`) `command` names one
site instead: a host name or IP address without scheme, port, path or wildcard. It matches
that host and its subdomains, without case: `{ "tool": "browser_*", "command": ["example.com"], "action": "deny" }`
denies every browser action on `example.com` and `login.example.com`, not `notexample.com`.
These tools check approval themselves once they know the site (for browser actions, the
current page), so a rule or grant always sees the real host. See [web tools](web-tools.md).
Allow and ask rules match the program token exactly (`/bin/echo` or a project-local
`./echo` does not match `["echo"]`, so an allow rule never widens). Deny rules also
match the program by basename, lowercased on macOS/Windows and without
`.exe`/`.cmd`/`.bat` on Windows, so `["rm"]` denies `rm`, `/bin/rm`, `./rm` and `RM`.
Deny rules also match every command a wrapper runs: `sh`/`bash`/`zsh`/`dash`/`ksh`/`fish`
`-c` scripts (split at `;`, `&&`, `||`, `|`, newlines, subshells and `$(...)`), `eval`,
`cmd /c`, `powershell`/`pwsh -Command`, and prefix wrappers (`env`, `sudo`, `doas`,
`nohup`, `nice`, `time`, `timeout`, `xargs`, `stdbuf`, `ionice`, `caffeinate`,
`command`, `exec`, `busybox`), nested up to four levels. So `["rm"]` also denies
`sh -c "ls && rm x"` and `sudo rm x`. Script files (`sh build.sh`) and other
interpreters (`python -c`, `node -e`) are not read: this is a tool policy, not a
sandbox; deny those interpreters if you need to.

`search_file` and `list_dir` do not consider `.gitignore`: `search_file` matches file names only (never contents) and looks at the first 100 entries it walks, `.git` and dependency folders included; `list_dir` leaves out dotfiles. Neither reaches a file's contents, so neither needs an approval for a sensitive file.

A deny rule holds across the two tools that run git: one on `shell` also covers the `git` tool, and one on `git` that names a `command` also covers a shell call running it (a `git` rule with no `command` stays about the tool). `git -C . push` is matched as `git push`. Allow and ask rules stay on the tool they name.

Project approval configuration may add deny rules only (including `command` denies). Those restrictions are
evaluated ahead of user rules; a project cannot grant itself access. Explicit
run overrides are caller-supplied policy. An interactive **Allow in this chat** grant
is saved for the chat (see below), applies at once to the active run and to every later
run of that chat, and cannot override a deny rule. Grants are keyed by the
action's grant key: the tool name (plus risk for configured policy) for most tools.
For `shell` the key is program plus first argument only for subcommand-style CLIs
(`cargo`, `git`, `npm`, `pnpm`, `yarn`, `go`, `rustup`, `pip`, `pip3`, `uv`, `poetry`)
whose first argument is a non-flag subcommand other than an executing one (`run`,
`run-script`, `exec`, `x`, `dlx`, `npx`, `create`, `tool`, `generate`, `shell`,
`install`, `add`). So approving `cargo test` covers `cargo test --lib` but not
`cargo publish` or `cargo run`. Every other program, wrapper and interpreter
(`find`, `sudo`, `env`, `ssh`, `bash -c`, ...) is keyed by its full argv. All tools of one
MCP server share one key (the server id inside the generated `mcp_<server id><tool id>_<tool>`
name), so one grant on any tool of that server covers its other tools; a different server,
or the same server after its plugin revision changes, asks again. Web and browser grants are
keyed by site: one grant on a browser action covers every browser action on that host, and a
`fetch_url` or `web_search` grant covers that tool on that host.

**Chat grants.** The approval box offers **Allow once**, **Allow in this chat** and
**Deny**. *Allow in this chat* runs the action now and saves the grant to the chat's own
file, `chat_grants/<chat id>.json` in the app data directory (beside `threads.json`). Format:
`{"version":1,"grants":[{"tool":"shell","command":["cargo","test"],"risk":"execute"}, {"tool":"mcp","server":"<server>_<16 hex>","risk":"network"}]}`;
`command` is the covered argv prefix (`shell`, `git` and `shell_unsandboxed`) or the one covered site (`browser`, `fetch_url`, `web_search`, e.g. `{"tool":"browser","command":["localhost"],"risk":"network"}`), `server` marks a whole-MCP-server
grant, and `risk` is `read`, `write`, `execute` or `network`. Limits: 256 grants, 256 KiB.
The file is loaded when a run starts (configuration and approval mode are still captured once
per run); a grant made during a run applies immediately to that run and is saved for later
runs. It is untrusted input: unknown fields, a wrong version, an oversized file, a destructive
command or any invalid entry make the whole file ignored (a fixed `chat_grants_ignored`
diagnostic with the reason, never the contents), which can only remove permissions. A chat's
automations and watches post into the same chat and inherit its grants; a watch's
`check_command` follows normal shell policy plus the chat's grants. Deny rules always beat a
grant, and YOLO is unchanged. Discarding a chat, or deleting its project, removes its file.
Neither `chat_grants/*.json` nor `mcp_consent.json` has integrity protection: both live in app data like the plugin registry. Sandboxed commands and Themis' file tools cannot write the app-data folder, but a command you let run outside the sandbox (or any command while the sandbox is off or unavailable) can edit them. Writes are serialized, merge with concurrent runs, and use unique temporary files.
**Queues and goals.** A chat's queued messages live in `queues/<chat id>.json` and its goal
in `goals/<chat id>.json` (app data, beside `threads.json`): `[{"id","text","attachments",
"reasoning_effort","held","note"}]` (at most 50 messages of 100,000 characters; `held` while being
edited or after the server refused it, with the reason in `note`; `reasoning_effort` applies only
while the chat's current model offers it) and `{"text","status","reason","started_at","active_ms",
"running_since","turns","failures","turn_seq","retry_at","evidence","also_asked","objective_updated"}`
with `status` one of `running`, `paused`, `waiting`, `blocked`, `quota`, `reached` (the goal text is
at most 2,000 characters; `also_asked` holds each message the person sent while it ran, word for
word, at most 50 of 4,000 characters; `evidence` is `[{"requirement","proof"}]`, at most 40 entries
of 300 and 600 characters, from `update_goal` complete; `objective_updated` is set by `edit_goal`
until the next goal run is told; `active_ms` is the goal's total running time, saved every minute while
it runs, so a crash or quit loses at most a minute of it). They grant nothing; a damaged file is ignored (fixed
`queue_ignored`/`goal_ignored` diagnostics, never the contents), and discarding a chat or its
project removes both. Diagnostics record queue and goal ids, status changes (`completed` and
`blocked` with the turn and evidence counts, `edited`, `also_asked` and `also_asked_full` with the
line count) and retry counts, never message text, the goal text or its evidence.
Goal notifications are sent by the desktop app (it watches `goal_changed` events and checks
its own window focus and the `goal_notifications` setting), which reports each attempt through
`record_notification` (`{"record":{"thread_id","kind","outcome"}}`, unknown fields refused):
service `notifications`, event `goal_notification`, with the chat id, `kind` (`reached`,
`blocked`, `waiting`, `quota`) and `outcome` (`sent`; or, at `warn`, `denied`, `failed`,
`unavailable`), never the chat title, goal sentence or reason.
There is no separate goal check: the chat's own model judges its goal, as in Codex. Every run
while the goal runs carries the goal note, appended to the run's message and never saved: a
goal turn's saved message is only `[Goal — runtime, not from the user] Turn N.` and any note
about the last turn (an interruption, a failure, open questions); a message of the person's
gets `goal-context.md` first. The note is `goal-continuation.md` with the objective (the text,
then each `Also asked:` line) inside `<objective>` tags as the user's data, preceded once by
`goal-objective-updated.md` after an edit. `create_goal` (description `tool-create-goal.md`) is
offered in every chat and refused while an unfinished goal exists; `update_goal`
(`tool-update-goal.md`) is offered while the chat has an unfinished goal: `complete` needs
`evidence`, one entry per requirement, and is refused without it or with an empty entry;
`blocked` needs a `reason`; `paused` is refused unless the person wrote since the current goal
turn started (`turn_seq`). Complete and blocked end the goal's turns and post a `goal_note`
(`reached`, `blocked`). Both tools need no approval prompt, but a deny rule for them wins. A goal
never stops on its own otherwise: failed turns retry with backoff however often they fail.
`goal_note` events show in the chat as plain lines: what the goal is doing and why, plus the
small-window and quota notes. Quota retries
(every 15 minutes) and failed-turn handling are fixed, not runtime configuration. A failed turn retries with doubling
waits from 30 s, plus up to a fifth of random extra. The goal pauses until resumed after 6 failed
turns in a row for errors that usually clear (network, 5xx, rate limits, empty replies), about 15
minutes of waiting, and after 3 for anything else. App server commands:
`list_queue`, `enqueue_message`, `update_queued_message`, `remove_queued_message`,
`steer_queued_message`, `get_goal`, `create_goal` (`threadId`, `text`), `edit_goal`, `pause_goal`, `resume_goal`, `remove_goal`
(also through `themis call`).
A steer is saved and sent as a `steer_shown` event at once; `steered` follows when the model reads
it, or `steer_returned` when the run ended first and the message went back to the queue. Clients
show one row for the pair, and only `steered` enters the model's history. Steering also denies every
approval the chat's run waits on, and any it asks for before reading the message, without a dialog;
each result tells the agent a new message came and it may ask again. The dialog's
`approval-expired` event carries `steered: true` (no expiry warning). Grants and rules do not change,
and neither a plain queued message nor an answer to `ask_user` denies anything. Diagnostics record
`queue`/`steered` with `approvals_denied`, a count.
**Questions.** The agent's `ask_user` tool (no approval prompt, like `create_goal`; a deny rule
for `ask_user` still wins) stores a chat's questions in `questions/<chat id>.json` (app data):
`[{"id","run_id","question","choices","assumption","reasons","blocking","topic","asked_at","answer"}]`
(`assumption` holds the tool's `recommended` answer, which the agent works on until answered;
`reasons` maps an answer to its one-line tradeoff, at most 160 characters, the recommended one's
from the tool's `why`, which a non-blocking question must give),
at most 8 open at once (the tool then tells the agent to keep working on its assumption) and
100 kept (the oldest answered go first); a question is at most 1,000 characters, with 2 to 4
choices of 200 (the recommended one first; a single choice becomes the suggestion), and an
answer at most 10,000. Asking again on an open `topic`, or with the same
wording, updates that question. The transcript records `question_asked` and `question_answered`
events so the chat shows them. A non-blocking question needs a `recommended` answer and the run goes on (a question with
a recommendation is never blocking, whatever the model asks for);
a blocking one ends the run after that tool batch with a fixed sentence, and in goal mode the
goal waits until that question is answered (answers to other questions run but leave it
waiting). `answer_question` (`threadId`, `questionId`, `answer`) delivers the answer like a
steer when a run is going (unread, it returns to the front of the queue), otherwise first in
the queue as the next message, joining an answer already waiting there; `list_questions`
(`threadId`) lists them. The answer message starts with a fixed marker the goal supervisor
reads. Open questions are listed in each goal turn's message. A
damaged file is ignored (`questions_ignored`); diagnostics record question ids, the blocking
flag and the delivery path (`asked`, `updated`, `answered`), never the question or answer text.
Answers grant nothing.
**To undo** a grant, edit or delete its entry in the chat's file, or start a new chat; there is
deliberately no UI for this, and the composer shield stays YOLO/Custom only.
Destructive actions (the variants below and the `destructive` risk class such as
`delete_file`) are never offered for the chat, never stored and never matched: the dialog
shows only **Allow once** and **Deny** with a fixed explanation, an "allow in this chat"
answer is downgraded to once, and an unattended automation run denies them without asking.

**Destructive variants.** Calls that discard work or cannot be undone are keyed by
their full argv and are never covered by a grant for their harmless sibling, nor by
an allow rule that does not name the variant itself:
- `git push` with `-f`/`--force`/`--force-with-lease`/`--mirror`/`--delete`/`--prune`,
  a `:ref` or `+ref` refspec; `git clean` with `-f`/`-d`/`-x`/`-X`;
  `git reset --hard|--merge` or `git reset <revision>`; `git checkout`/`switch` with
  `-f`/`--force`/`--discard-changes`/`-B`/`-C`, and `git checkout` of paths
  (`.` or anything after `--`); `git restore` of the working tree (only `--staged`
  alone is safe); `git branch` with `-D`/`-f`/`-M`/`-C`; `git tag -d|-f`;
  `git commit --amend`; `git stash drop|clear`; `git reflog expire|delete`;
  `git worktree remove --force`; `git gc --prune`; and `git rebase`, `update-ref`,
  `prune`, `filter-branch`, `filter-repo`.
- Git accepts unambiguous long-option prefixes (`--forc`), so any prefix of a listed
  long option counts. Global options with values (`-C dir`, `-c k=v`, `--config-env n=E`,
  `--git-dir`, ...) do not hide the subcommand.
- `publish`/`unpublish`/`deprecate` for `npm`, `pnpm` and `yarn` (also behind options
  such as `pnpm --filter pkg`, `npm -w x`, `yarn npm publish`, and npm's command
  abbreviations of at least three characters such as `npm publi`), and `publish`/`yank`
  for `cargo` (also `cargo +nightly publish`, `cargo --config k=v publish`), including
  `--dry-run`. Positional words are scanned until an ordinary subcommand (`test`, `build`,
  `install`, ...) is reached, so `cargo test publish` or `npm test publish` do not count,
  while `npm run publish` (a script that likely publishes) still does. A word right after
  a flag may be that flag's value and never ends the scan: over-flagging costs a prompt only.
- Recursive `rm` (`-r`, `-R`, `--recursive`, or a cluster such as `-rf`).

These variants carry the risk class `destructive` for `shell` calls too (the `git` tool
also escalates destructive `checkout`, `branch` and `commit` calls, and its `status`, `diff`,
`log` and `show` calls with only output-shaping flags carry `read`; other `git` calls `write`), so a rule with
`"risk": "execute"` does not cover them. So a rule `["git","push"]` allows
`git push origin main` but `git push --force` falls through to later rules and the
default (ask; denied when unattended). A rule such as `["git","push","--force"]` is
honoured. A broad allow rule without `command` (`{"tool":"shell","action":"allow"}`, or
`"tool":"*"`) is trust in the tool but not in its destructive variants; add
`"risk": "destructive"` to such a rule if you really mean them. Wrapped commands count too
(`sudo rm -rf x`, `bash -c "git push -f"`, `env X=1 git reset --hard`), and so does a
wrapped script that cannot be read reliably (unbalanced quotes, `$(...)` inside a quoted
word, a variable as the program, `-EncodedCommand`, more than four wrapper levels), so
those ask every time and are never covered by a chat grant.

**Deny rules compare flags by meaning.** A deny rule's non-flag tokens must begin the
command's non-flag tokens and every flag it names must appear before `--`, in any order,
as `--name=value`, inside a short cluster, or under a known equivalent spelling:
`git` `--force`/`-f`, `--delete`/`-d`, `--all`/`-a`, `--recursive`/`-r`; `rm`
`--force`/`-f`, `--recursive`/`-r`/`-R`, `--dir`/`-d`; `cp` `--force`/`-f`,
`--recursive`/`-r`/`-R`; `mv` `--force`/`-f`; `chmod`/`chown` `--recursive`/`-R`.
So denying `["git","push","--force"]` also denies `git push -f origin main` and
`git push origin --force`, and `["rm","-rf"]` denies `rm -fr x` and `rm -r -f x`. Other
programs' short and long spellings are not related (their meanings differ per program):
list each spelling, or deny the broader prefix such as `["git","push"]`. Allow and ask
rules stay exact so they never widen. The `git` tool reports `["git", args...]` as its
command with the same keys.

The `shell` tool schema asks for the program only in `command` and one `args`
element per argument. If `command` contains whitespace and no executable of that
exact name exists, the error returns the exact split (unless the first token is a
nonexistent path, such as a path containing spaces, which gets a spelling hint instead) (`command: "python3", args:
["-m","http.server","8798"]`). The call is never auto-split or rerun, so approvals
always see the argv the model sent. One shape is repaired before approval: when a
`shell` or `git` call sends its `args` list as one string holding that list as JSON
(`"args": "[\"-L\", \"x\"]"`), the list is read from the string, so approval,
the transcript summary and the run all use the same argv. Any other string `args`
is rejected as before.

Unattended CLI `ask` requests deny; use explicit policy or an interactive client.

YOLO means no approval prompts and no sandbox walls but Themis' own settings: it
bypasses harness approval gates (but not `deny` rules: a matching deny rule still
refuses, while `ask` rules and the default allow), filesystem root restrictions,
skill tool filters, and the Git subcommand allowlist, and its commands may write
anywhere, read secret locations and use the network and Keychain unless the
configuration wrote a block for them. Writes to the app data and the
project's `.themis/` stay refused (commands and file tools alike; under Landlock
only the file tools protect `.themis/`), and a YOLO run's system prompt carries a
caution against unrequested destructive actions (`desktop-run-yolo.md`).
Operating-system permissions, tool input validation, time/output bounds, and child
environment handling still apply; a command blocked by that one wall fails without
asking. Custom is a tool policy on top
of the same sandbox: approved programs, including an explicit shell interpreter,
perform their own filesystem/network operations inside it. See
[sandbox](sandbox.md) for the enforcement boundary.

## Effective permissions view

`get_effective_permissions` (`projectRoot`, optional `threadId`) returns a
read-only view built from the files on every call: the chat's approval `mode`,
`files` (user and project file paths with `status` `loaded`, `missing`, `invalid`
or `unreadable` and the first `error` with `line`/`column` when known),
`validation_error` for the resolved configuration, `approval` (`rules` in
evaluation order, each with `source` `default`, `user`, `project` or `run`, the
`default` action with its source and `project_rule_count`), `sandbox` (state,
chosen backend and why, network and whether it is enforced, writable, protected
and secret paths, every backend's availability, what is and is not covered, and
the source of each sandbox field) and `chat` (the chat's grants and the grants
file path). `themis permissions [PROJECT] [--thread ID] [--json]` prints it (the
current directory when neither is given); Settings → Permissions uses only its
global problems. `ensure_runtime_config` returns the global `runtime.jsonc` path,
first writing the starter when the file does not exist or still sets nothing
(never changing a file with a setting in it) and refreshing `runtime.reference.jsonc`; `open_runtime_config` does the same and
opens that file as plain text in the system text editor. `get_runtime_configuration` also
returns `rule_sources` beside the resolved configuration.

## Diagnostics

Logs stay out of the desktop UI. Advanced users can query structured records
through the CLI/API or inspect the local JSONL files. Filters accept exact
severity, service, or operation ID.

Records contain epoch-millisecond timestamp, severity, service, event, and
metadata. Events cover server lifecycle (`server`/`started`, `stopped`, and `discovery_lost`
when a server stopped because its `server.json` was gone), configuration saves, integration
inspection/import/test outcomes, approvals (`approval`/`decided`: each gated action's
`outcome`, `allowed` or `denied`, whether a rule, a chat grant, YOLO or a person settled it;
`approval`/`answered`: only when a dialog was shown, what the person chose: `once`, `chat`
for Allow in this chat, `deny`, `new_message` when they steered a message past it, or `none` when it expired, with `waited_ms`), the sandbox state each run
captured (`sandbox`/`run_plan`: state, backend and network, never paths or
commands), and compaction stages. A failed server command (`server`/`command_failed`)
records its method and a fixed `category` (`unknown_method` when a window and the
server come from different builds, `too_large`, `invalid_request`, `timeout`,
`not_found`, `conflict`, `denied`, `network`, `credentials` or `other`), never the
error message, which can name paths or hold user text.
Request bodies, prompts, tool arguments/results, and provider bodies are excluded.
Known credential fields and referenced secret environment values are redacted;
debug subprocess stderr is bounded and filtered. Debug output from third-party
programs may contain other private information; review exports before sharing.

Three files retain at most 2 MiB each. Individual records are capped at 16 KiB;
queries default to the latest 200 records and support up to 2,000. Rotation is
local retention, not a permanent audit archive. Severity thresholds can suppress
lower-severity records, including info-level approval events. CLI/API queries
return the filtered records. An explicit MCP test checks only that invocation; it does not establish
continuous health or guarantee future connections.

### Resource use

The server samples Themis's own process tree every 5 seconds: the app (the
window process and the server), the built-in browser (Chrome and its helpers)
and tools (dev servers, MCP servers, background commands and anything they
start). CPU is CPU time spent between samples over the wall time (percent of
one core, summed); memory is each process's physical footprint on macOS (as in
Activity Monitor) and resident size elsewhere. The WebKit process that draws the
window is started by macOS, not by Themis, and is not counted. One hour of
samples stays in memory.

- `resources`/`sample` (info, once a minute): CPU and memory per group, core
  count, and the Workspace browser stream's counts for that minute (`starts`,
  `stops`, `frames`, `lagged_events`, `restarts_after_lag`).
- `resources`/`memory_growth` (warn, at most every 30 minutes per group): a
  group whose memory rose steadily over the last 30 minutes — every sample of
  the last 10 minutes above every sample of the first 10, and the averages at
  least 64 MiB apart. A sawtooth (memory freed now and then) or a single spike
  does not count.
- `themis call resource_usage '{}'` returns the latest sample: `cpu_percent`,
  `memory_bytes`, `cores` and `groups` (`app`, `browser`, `tools`, each with
  `cpu_percent`, `memory_bytes`, `processes` and, when growing, `growing_bytes`),
  or `null` before the first sample.

The resolver and read/query APIs are available through the actual CLI:

```sh
themis call get_runtime_configuration '{"projectRoot":"/path/to/project"}'
themis call query_diagnostic_logs '{"service":"integration","limit":200}'
themis call query_diagnostic_logs '{"operationId":"OPERATION_UUID"}'
themis call query_diagnostic_logs '{"service":"resources","limit":60}'
themis call query_diagnostic_logs '{"service":"notifications","limit":50}'
themis call resource_usage '{}'
```

`save_runtime_configuration` accepts `scope` (`user`/`project`), optional
`projectRoot`, and `json` (a JSONC string). `set_thread_approval_mode` accepts
`threadId` and `mode` (`custom`/`yolo`). `send_message` optionally accepts
`runtimeConfiguration` as a JSONC string for that run. Saves validate the resolved
configuration and referenced prompt before atomically replacing the file.

## Contributor verification

Keep config validation/schema and runtime resolution in core; reuse the shared
server from both frontends. Test precedence, invalid input, project confinement,
policy denies, and immutable run behavior when changing those contracts.
Use synthetic credentials and isolated app data. The production-server/CLI
regression is `cargo test -p themis-desktop --test configuration_diagnostics`.
Core checks include `cargo test -p themis-core --lib`; UI tests are adjacent to
the composer, Integrations, and bridge.

Automated tests prove their exercised paths. Native verification, third-party
network acceptance, and model retention benchmarks are separate evidence;
previous benchmark results are dated reports, not automatic runtime audits.

## Background MCP connections

The app starts enabled global and selected-project MCP connections asynchronously on entry. Enabling a connection authorizes initialization and tool discovery; imported connections still start disabled. Consent is bound to the exact definition: Themis keeps a fingerprint (command, args, url, environment references, bearer variable, and the plugin's resource files and their executable flags, bounded to 1,024 files / 8 MiB, over which consent fails closed) of what a person approved in `mcp_consent.json` beside the plugin registry. A person saving or enabling the connection in the UI or CLI records it; so does answering the dialog for an agent's change, and only while the stored definition still matches what that dialog showed. Agent management calls on one plugin run one at a time. An agent change applied under YOLO or an allow rule takes effect but records no consent: the connection shows "Needs your approval" until you enable it. When the sole executable effect of an agent's save is one command-based connection (no hooks or skills), the single argv dialog replaces the generic one; otherwise both appear. Chat grants for an MCP server also expire with its definition (the definition is part of the server id). A definition that is new or changed by anything else (a repository's `.themis/plugins/` or `.themis/config.jsonc`, a catalog update or import, an unapproved edit) shows "Needs your approval" and does not start, in the background or in a run, in Custom or YOLO, until a person enables it again; imports still arrive disabled. On the first start after upgrading, already-enabled global definitions are seeded as approved; project definitions never are. An agent that creates, edits or enables a command-based connection through `manage_integrations` is asked about the exact command (shown as structured argv, one connection per call) every time; that dialog offers only Allow once and Deny and is never remembered for the chat, YOLO skips it as the user's explicit trust, and a deny rule refuses it. Runs no longer show a "Connect MCP server" prompt: explicit user/project deny rules for `mcp_start_*` (or the server) still prevent startup, in the background and in a run, and YOLO ignores rules as before. Tool calls are separate approvals; one **Allow in this chat** covers the server's other tools. Calls from a chat continue to use that run's captured approval policy and approval mode, including when they reuse an existing transport. Startup has a 30-second deadline per server and failures are isolated. Definitions remain in the existing integration registry; transports and observed status are ephemeral shared-server state. Changing or disabling a definition releases its idle connection while active runs retain their immutable snapshot.

The shared `plugin_action` API accepts `mcp_status` with `projectRoot`. `connect: true` reconciles enabled definitions and starts missing connections without waiting; `retry: true` also retries failed connections. Without those flags it only reads observed status. This supports CLI verification through `themis call plugin_action`. App entry, project selection and server reconnection trigger initialization automatically.

Agent delivery limits (16 KiB per result, 48 KiB per batch, 64 KiB per serialized tool call, each reduced for small context budgets) are runtime safeguards rather than new configuration fields. The context estimate includes tool schemas and an answer reserve. Reads default to 8,000 content characters, and repeated archive recovery is bounded within each run. See [context limits and recovery](context-and-attachments.md) for offloading, errors, and the distinction between model delivery and source/tool capture limits.

## Context accounting API

`logging.context_metrics` is captured once per run. `off` disables optional request accounting; bounds, evidence saves and loop guards remain active. `summary` retains aggregate counts and the latest request; `detailed` also retains the last 32 request records with mutually exclusive role and purpose distributions. These are alternative views of the same message bytes, never additive. Estimates use serialized bytes and message overhead, not a verified model tokenizer. Within a run, budget decisions scale the estimate by the ratio of provider-reported to estimated input tokens (clamped 0.75-4.0, no change until a request reports usage) and a context-length-exceeded error triggers one compaction and one retry; `token_calibration` samples in the snapshot show estimate versus report per request. No new configuration fields; see [estimate calibration](context-and-attachments.md#estimate-calibration-and-overflow-recovery). The live catalog is joined to models.dev context/output limits. Ordinary requests clamp to the selected model's published capacity; internal summaries separately clamp to the effective nonthinking summary model. Unknown production capacity starts from a 128,000-token guess (`capacity_source` `guess`) and learns the real limit from a context-length error (`learned`); see [Context budget](#context-budget). Published metadata is not proof of successful live requests at that capacity.

Every actual provider attempt, including internal summaries and retries, contributes to request counts and latency. Reported input/output totals remain separate from cached-input and reasoning-output subcounts, which are included in normalized parent counts. Snapshots also total those subcounts over the run (`provider_cached_input_tokens`, `provider_reasoning_output_tokens`; zero when the provider reports none) and the byte estimate of every request's input (`estimated_input_tokens`), which the budget eval uses to estimate spend. Anthropic uncached input, cache reads and cache creation are combined once; OpenAI cache detail is already included in input. Missing usage is null/unavailable, with missing-request counts; it is never reported as zero. Partial streams retain reported usage when available, and cancelled/failed requests remain in accounting. Streaming Chat Completions requests ask for usage and consume trailing usage frames. Run snapshots carry no cost; Settings › Usage shows labelled list-price estimates (see [token usage records](#token-usage-records)).

Metadata snapshots persist as context_usage events in the existing sessions database. The final accounting snapshot can follow the terminal event so cancelled request cleanup is included. Query without reading conversation payloads:

```sh
themis call get_context_usage '{"threadId":"THREAD","runId":"RUN"}'
```

Omit runId for the latest snapshot; no available snapshot returns null. Diagnostic JSONL remains bounded and rotating as documented below. It records correlation IDs, pipeline/job/stage/attempt, model, sizes, estimates, provider usage, timing and outcome categories. It excludes prompts, filenames, arguments, results, document passages, provider bodies and credentials. Historical source excerpts and stage notes belong only in transcript/evidence storage. This provides backend data for a future dashboard; no raw-log viewer or new dashboard UI is added. Usage and runtime events are observations, not automatic judgments of narrative or coding correctness.

## Token usage records

Every completed model request is recorded once in app-data `usage.sqlite3`, whatever
`logging.context_metrics` says: time, provider id (`opencode-go`), model id, kind
(`answer` for conversation turns including spec checks, `summary` for checkpoints and
document capsules, chat and run id, and the
provider-reported input, cached input, output and reasoning tokens (null when the
provider reported none). Input includes cached input and output includes reasoning, as
in the context accounting above. Failed and cancelled requests are not recorded. No
prompt, reply, file name, tool argument or credential is stored.

- Recording never waits on the database: rows go through a bounded queue (1,024) to one
  writer thread, in a separate file so a usage write never takes the conversation
  store's lock. A full queue or a failed write drops the row and logs
  `usage`/`rows_dropped` or `usage`/`write_failed` (counts and error kinds only); a file
  that cannot open logs `usage`/`store_unavailable` and the Usage section says so.
- Rows older than 35 days are rolled into daily totals per local day, provider, model,
  kind and chat; daily totals older than 400 days are deleted. The writer rolls up when
  it starts and every 500 rows. Deleting a chat keeps its usage (listed as Deleted chats).
- Only requests this Themis made are counted. Another app or device using the same
  account is not, so the provider's own figures can be higher.

`get_usage_summary` (no arguments) returns, in this computer's time zone: `windows`
(`five_hours` = the last 5 hours, `today` from local midnight, `week` from Monday 00:00,
`month` from the 1st), each with `requests`, `unreported_requests`, the four token sums,
`estimated_cost_usd` (absent when no request was priced) and `unpriced_requests`;
`days` (the last 30 local days, oldest first, input + output tokens per model);
`by_provider`, `by_model` (with `free` for zero-priced models) and `by_chat` (top five
existing chats with titles, then one `key: null` row for deleted chats), all over the 30
days; and `billing`, one entry per provider with `kind` (`plan_windows`,
`pay_per_token`, `free`, `local`), `windows` (verified plan limits; always empty today)
and `price_source`. A day whose midnight is skipped by a clock change starts at its first
existing hour; a repeated midnight uses the earlier one. `get_chat_usage` with
`threadId` returns one chat's totals, raw rows plus daily totals.

Estimated cost uses the per-model list prices (`cost.input`, `cost.output`,
`cost.cache_read`, and context tiers by input size) in the public models.dev catalog that
the model list already reads; cache writes are priced as input. It is an estimate of
list-price usage, not a bill: OpenCode Go charges a flat fee with usage limits per 5 hours,
week and month, and Themis shows no allowance or percentage because those limits are not
published in a verified form. Models the catalog does not price are counted in
`unpriced_requests` and left out of the estimate.

```sh
themis usage          # the same windows, models and chats as plain text
themis usage --json   # get_usage_summary
themis call get_chat_usage '{"threadId":"THREAD"}'
```

## Memory

Memories live in the app data folder, never in a project (the project's shared rules stay
in its `AGENTS.md`): `memory/global/` for memories that apply everywhere and
`memory/projects/<hash>/` per project, where the hash is the first 16 hex digits of the
SHA-256 of the project's canonical path (renaming a project keeps its memories). Each memory
is one Markdown file, `<name>.md`, with a header of `name` (lowercase kebab-case, at most 64
characters), `description` (one line, at most 160), `type` (`user`, `feedback`, `project` or
`reference`), `saved` (`YYYY-MM-DD`) and an optional `source` (at most 200; the web page or
file it was built from), then the text (at most 4 KiB; feedback and project memories end with
`Why:` and `How to apply:` lines). Files are read as untrusted: a file with another key, a
key twice, a bad name or date, control characters or more than 8 KiB is skipped, and
symlinks are never followed, for the folder or a file.

Beside them, `MEMORY.md` is the index: one line per memory (`- name (type, date[, from
source]): description`), newest first, at most 200 lines and 24 KiB. It is rewritten
atomically on every save and delete. A run reads the global index and the current project's
(never another project's), and in full the memories relevant to its message (the person's
words, or the task for a goal turn or automation): the index lines that share the most words
of at least four letters with it (common filler words left out, a plain plural counted as its
singular, so "games" finds "game"), at most 5, this project's first on a
tie, within 8 KiB together. That rule needs no model request and reads two index files and at
most five memory files per run, whatever the memory count. All of it goes into the system
message of the main agent and of every subagent under the note in
`crates/core/builtins/prompts/system-memory.md` (the full memories under
`system-memory-relevant.md`). Nothing is added while both indexes are empty. The context
meter counts that block, as sent, as its own **Memory** part (`breakdown.memory`, taken out
of `system`, so the parts still add up to `used_tokens`).

**`search_memory`.** While memory is on, the main agent also gets a read-only `search_memory`
tool (description `tool-search-memory.md`) for what the injected index and full memories
don't cover. `query` returns at most 10 lines, scored by the same word rule, across this project's
memories, the global ones, this project's plan records and its tasks outside any record (up to
3 places kept for tasks), each with its source; `name` returns one memory (this project's
before the global one) or plan record in full, and `tasks` the whole task list. It
reads only the indexes and one file, never another project's memory, writes nothing, and
approves itself (a deny rule still refuses it). `logs/` gets a `memory` / `searched` line with
whether it searched or read and whether it succeeded.

Only `user` and `feedback` memories may be global; `project` and `reference` memories always
belong to one project. A scope holds at most 200 memories. A memory built from a web page or a
file keeps it in `source` and says what that content said, never as the person's word.

### Plan records

Plans are not memories. Each piece of work the person approved has one plan record,
`memory/projects/<hash>/plans/<name>.md` (project only; at most 50 per project, 64 KiB per
file; a record trims its oldest closed tasks, then its oldest decisions and changes, its last
evidence lines, lessons and results, rather than grow past that), with a header of `name`, `title` (one line, at most 120), `status` (`approved`,
`in progress` while a goal builds it, `done`), `updated` and, while a chat's goal builds it,
`chat` (that chat's id), then five sections: **Plan** (the reply the person approved, copied
word for word by Themis, at most 8 KiB), **Changes** (one line per change, at most 20),
**Decisions** (the person's only, at most 12 lines: what was chosen, `— because` why, and the
date; the oldest go first), **Tasks** (task lines, below), **Results** (at most 2 KiB),
**Evidence** (one line per requirement, at most 12 of 1000 characters) and **Lessons** (at most 3).
Decisions and Tasks may be missing from a record written before they existed; it still reads. `plans/PLANS.md` is their index (`- name (status, date): title`).
Records are read as untrusted: a file whose header or sections don't fit is skipped, secrets
are hidden, and the sections are found from the end so a plan that quotes a heading keeps it.

The memory pass manages them through one `"plan"` entry beside `"memories"` in its answer:
`create` (the person approved the plan in the reply they answered: Themis copies that reply;
the model only names it), `change` (a change line plus `old` → `new` edits, each applied only
when `old` occurs exactly once in the plan, so every step the change doesn't touch stays word
for word), `done` (results and lessons; evidence only when the record has none) and `delete`.
When a goal completes, Themis itself marks the record its chat's goal built as done and copies
the goal's evidence as the agent gave it (`memory` / `plan` with `by: goal`), before the
goal-complete pass adds results and lessons. Ordinary passes never save a plan, its steps or
what was built as memories. A run gets, after the memories, the plans index and the
record its chat's goal builds, else the one whose line shares the most words (at least two)
with the message (`system-memory-plans.md`, `system-memory-plan.md`): its plan, changes,
decisions, tasks (open ones in full, the latest 20 cancelled ones with why, done ones by
title, the latest 20) and lessons, while
results and evidence stay in the file for `search_memory`. One index file and at most one
record per run.

### Tasks

Tasks keep track of the work along the way. A task line is `- [status] title — note (date)`:
status `open`, `doing`, `done` or `cancelled`, a title (at most 120 characters), a note (at
most 300: what done means and the person's own words, or the outcome and its proof, or why it
was cancelled) and the day it last changed. Themis makes each step of an approved plan (each bullet or numbered line outside code blocks
and outside a section of open questions, a `**Label:**` step titled by its label) an open task
when the record is created and when a change adds steps (never for work already under way,
whose steps' state it can't know); the memory models only move
their statuses, recording progress on a task worked on but not finished as `doing` with what was
tried. Tasks of approved work live in its record's Tasks
section; the rest in one project file, `memory/projects/<hash>/plans/TASKS.md` (at most 48 KiB).
A list keeps at most 40 open or doing tasks (a new one past that is refused, never an open one
dropped) and 60 done or cancelled ones (the oldest go first); a task is never renamed or
removed otherwise. The memory pass reads the record's tasks and the project file
(`memory-auto-user.md`, at most 16 KiB) and answers with at most 12 `"tasks"` ops: `add`
(title, note, optional `status`: open by default, or doing or done for an ask the turn already
started or finished, so finished asks stay on record; optional `work` naming a record; a title
already listed updates its note and reopens it) and `update` (exact title, status, note), and at most 3 `"decisions"` (`work`,
`what`, `why`; the person's choices only, never the agent's own) for records. Entries holding a secret or not fitting are
refused. When a record is marked done, Themis closes its tasks still open or doing as done, saying in
the note they were closed when the work was marked done (the evidence is the proof). The
goal-complete pass reads the goal's working log (failed steps and the agent's notes, the latest
40) for its lessons. Tasks and decisions carry their date: runs are told an older open task may
since be done or no longer wanted (check the files, and ask before starting one this chat didn't
raise) and that the latest decision on a matter wins; passes close an open task the turn shows
already done or no longer wanted. A run gets the project file's tasks the same way: open ones in full, the latest 20
cancelled ones with why, and the latest 20 done ones by title (`system-memory-tasks.md`). `logs/` gets a `memory` / `tasks` line with counts only
(applied, missed, decisions, failed); task changes stay out of the chat's memory line. The pass gets the same index and record. `logs/` has `memory` /
`plan` lines with the op and outcome (`saved`, `updated`, `removed`, `exists`, `missing`,
`invalid`, `sensitive`, `no_plan_reply`, `failed`) and edit counts only. The chat's memory
line shows a record as `plans/<name>.md`.

### Memory jobs and the memory models

Memory is kept by background jobs on the memory models (`memory.models`), never by the chat's
model, which has no memory tool:

- **The pass**, when a turn's reply is done and the person said something in that turn (the
  message that started it, what they steered in, an answer to the agent's question; in a goal,
  after each goal turn they spoke in, except words a goal event below already passed on; never
  a hook's wake or an automation), except a trivial
  one (under 8 characters, such as "ok" or "thanks"). One request with no tools and the
  least thinking the model allows reads the reply their message answered, what they said (at
  most 8 KiB), the reply that ended the turn (each reply at most 4 KiB), both indexes and, in
  full, the memories the turn may touch (at most 5, 8 KiB, chosen like a run's), so an update
  keeps what still holds, with the prompts `memory-auto-system.md` and `memory-auto-user.md`,
  and may save, update (with an optional `new_name`) or delete at most 3 memories. It runs on
  its own task and never holds up the next message. When it changes memory while that chat's
  next run already works, that run gets the changes at its next step as a runtime note
  (`memory-changed-note.md`).
- **Event passes**, on events that matter without the person's words, each with the event's
  content (at most 8 KiB): a goal created, by `/goal` or the agent's `create_goal` (its
  objective, the person's latest message and the reply it answered, so an approved plan is
  kept before the goal's first turn ends), a message the person steered or sent into a running
  goal that became an "Also asked" line (their words and the objective), a goal completed (its
  objective, "Also asked" lines, evidence and the chat's latest checkpoint summary, from which
  at most 2 lasting lessons are kept as rules), the goal edited by the person (its new
  objective). A context checkpoint starts none: its summary is mostly the state
  of unfinished work, which goes stale within minutes. `logs/` has `memory` / `event` with the
  event name.
- **Consolidation**, when the app has been idle (no run going) for 5 minutes after a job, at
  most once per `memory.consolidate_every_hours`: one request per scope with at least two
  memories (the global scope, then each project with a pass since the last one), with every
  memory of that scope in full (each text at most 1 KiB, the whole at most 48 KiB), the prompts
  `memory-consolidate-system.md` and `memory-consolidate-user.md`, and at most 10 changes,
  only in that scope: it merges duplicates into one and deletes clearly stale ones.

**When a pass runs, and what it reads.** Besides the person's words, every pass reads what the
turn ran into, in order from that turn's history: each failed step (tool, end of its output, at
most 300 characters) and each note the agent wrote between steps (what it tried, found wrong,
dropped or thinks could be better, at most 400 characters), the latest 40 entries; the note that
ended the turn is its reply and is not repeated. A goal-complete pass reads the goal's failed
steps with what the agent said next (at most 8). A turn the person said nothing in (a goal turn)
still gets a pass when it has such a log, since that is when a lesson is fresh. Every pass also reads the chat's latest answered question (at most 2 KiB) when the turn doesn't already carry it, marked as read before, so a pass that missed what an answer settles (a task it proves done) can still close it, without saving what is already remembered again; the prompt keeps lessons as project or feedback memories
(symptom, cause, rule, how to check) worded so a similar later task finds them. `memory.thinking`
(default `auto`, user file only) sets how much the memory models think: `auto` lets them think for
a pass that carries lessons (a turn with work in it: failed steps or notes, a completed goal) and asks for the
least otherwise; `least` always the least; `model` always the model's own. The least effort comes
from the Go catalog (loaded first when empty), so a model that refuses `none` gets its lowest
listed effort. Measured with `python3 -m evals.memory` (`memory_try_pass`, a read-only dry run of
one pass through the CLI: `{projectRoot, person, answered, reply, ran_into, qa_fixed, event,
thinking, model}` → the answer, what it would save, time and tokens; it saves nothing).

Each job tries the models in order and moves on after a provider error, a time-out, a rate
limit, an answer it can't read, or the daily cap. A job every model failed waits in memory
(at most 8 jobs; a ninth pushes the oldest out) and is tried again once the app is idle, 1, 2,
4, … minutes later (at most 6 hours apart), at most 6 times; a job held back only by the cap
waits until tomorrow without using up its tries. Waiting jobs do not survive a restart.
Settings › Memory shows "Waiting for the memory models: N jobs (…)" with the reasons while any
wait.

The answer is untrusted: `{"memories":[{action, scope, name, type?, description?, body?, why?,
how_to_apply?, source?}]}`, at most 16 KiB. `delete` needs only `name` and `scope`. An entry
with an unknown action or type, a bad name or no text is dropped; one that holds a secret
(anything the secret redaction would hide, in any field) is refused outright, not saved
redacted; a `project` or `reference` memory proposed for every project is refused. Against
what is remembered, a delete of a memory that isn't there is dropped, a save of a name the
scope already has becomes an update and an update of a name it lacks a save, and an entry
whose name, description and (when read) text are already there, or whose description another
memory not being deleted already has, is a duplicate and skipped.

Each change a pass makes is one saved `memory_changed` chat event under that turn's reply:
`changes`, each with `action` (`saved`, `updated` or `removed`), `scope`, `name`,
`description`, `model`, `model_index` (its place in `memory.models`, from 0) and
`project_root` for a project memory. Consolidation changes no chat. Tokens are recorded as
`memory` usage (Settings › Usage), with the chat for a pass.

Diagnostics (metadata only, never what was said or saved): `memory` / `job` for each model
tried, with the job `kind` (`pass` or `consolidate`), `model_index`, `outcome` (`answered`,
`rate_limited`, `provider_error`, `timed_out`, `unreadable`, `over_budget`, `unavailable`),
input bytes, provider token counts and time; `memory` / `job_done` with the outcome (`changed`
or `nothing`) and counts (`proposed`, `saved`, `updated`, `deleted`, `duplicate`, `missing`,
`sensitive`, `scope`, `invalid`, `over_limit`, `failed`); `memory` / `job_waiting` with the
reasons and how many wait; `memory` / `job_dropped`; `memory` / `pass_skipped` with the reason
(`memory_off`, `no_person_message`, `trivial`); `memory` / `consolidation`; and `memory` /
`saved`, `updated` or `deleted` with the scope, type, the scope's count and what did it
(`pass`, `consolidate` or `settings`).

Shared-server commands: `list_memories {scope, projectRoot}` returns the scope's memories,
`index_lines`, `max_lines`, `skipped`, `enabled`, `waiting` and a `revision`; `delete_memory
{scope, projectRoot, name, revision}` refuses a stale revision; `get_memory_settings` returns
the `memory` block, its `revision` and `waiting` (`null`, or `{jobs, reasons, next_at}`);
`update_memory_settings {enabled?, models?, revision}` writes `memory.enabled` and
`memory.models` in place and refuses a stale revision or a list the resolver refuses.

## Shared-server event stream additions

Besides thread, approval and review events, the server broadcasts `thread-changed` (`{kind: created|updated|removed, project_root, thread_id, thread?}`) after a successful `create_thread`, `rename_thread`, `set_thread_approval_mode`, `set_thread_effort`, `set_thread_skills` or `discard_thread`, so an open desktop window follows changes made by `themis call` or another client. It carries metadata only. The `approval-request` event carries `grant_scope` (`{argv, covers_further_args}`, shell only): the raw command prefix an "allow in this chat" grant covers, matching the stored grant key. It also carries `grantable` (`false` for destructive actions, which only offer Allow once and Deny) and the `approve_action` decision is `once`, `chat` (formerly `always`, still accepted) or `deny`. When a dialog times out the server emits `approval-expired` (`{thread_id, approval_id}`) after denying it, so clients remove the stale dialog. Streamed assistant text is persisted before a run's terminal event, including Stopped and failed runs.
