# Configuration, scope, permissions, and diagnostics

Desktop and CLI use the same local app server. Runtime configuration is an
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
| App-data `runtime.jsonc` | Runtime defaults, Custom approval rules, OS sandbox, logging, discovery | User, across projects |
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
The existing nonblank `OPENCODE_KEY` fallback remains available when the OS store
has no accessible key. A valid stored key keeps precedence. No authentication is
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
unsupported schema versions, invalid types, and out-of-range values fail.
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

Optional `context_token_budget`, `recent_messages`, and `total_turns` override
the existing Settings defaults. Omit them to retain those settings.

Example user `runtime.jsonc`:

```jsonc
{
  "schema_version": 1,
  "context_token_budget": 200000,
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
    "section_max_tokens": 4096,
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
(`shell`, background tasks, the `git` tool and watch checks), in Custom and YOLO
alike; see [sandbox](sandbox.md) for what it allows, the secret locations it
hides, backend choice and limits. A project file and an explicit run override may
only tighten it (block the network, enable it, drop writable paths); anything
else fails validation with a message naming the field.

`skills.paths` replaces project skill search roots; it does not change conventional
user discovery. Skill and instruction paths must be relative, without parent
traversal, and remain inside the project after canonicalization. Each list permits
at most 32 paths. Instructions are UTF-8 project guidance, limited to 64 KiB
combined, and do not grant permissions. There is no implicit instruction globbing.

## Supported limits

| Field | Range/default |
| --- | --- |
| `context_token_budget` | 2,000–256,000; omitted uses Settings (default 200,000) |
| `recent_messages` | 2–200; omitted uses Settings |
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
| `compaction.max_stages` | 1–128; default 32, including consolidation |
| `compaction.retry_limit` | 0–2; default 2 (up to three attempts per failed stage) |
| `compaction.section_max_tokens` | Legacy compatibility field, 256–16,384; retained for file compatibility; stage output is computed from consolidation capacity |
| `compaction.document_context_token_budget` | Internal full-document preparation input budget, 24,000–1,000,000; default 200,000; clamped to the routed summary model capacity, with conservative 24,000 when unknown. Does not change ordinary context capacity. |
| `compaction.summary_max_tokens` | Initial summary-output ceiling, 256–16,384; default 8,192. Retries may use up to twice the effective initial allowance, subject to capacity. |
| `compaction.summary_model` | Optional `longcat-2.5-preview-free`, `gpt-5.6-luna`, or `gpt-6-luna`; omitted/null keeps automatic routing. Only internal summaries change; the configured model must be present in the captured Go catalog. |
| `compaction.prompt_file` | Optional UTF-8 Markdown, at most 32 KiB |
| `approval.rules` | At most 256 ordered rules `{tool, action, risk?, command?}`; `command` is an argv-prefix array (see Composer permissions) |
| `sandbox.enabled` | Boolean, default `true`; only the user file (or no file) can turn it off |
| `sandbox.backend` | `auto` (default), `seatbelt` (macOS), `bubblewrap` or `landlock` (Linux); a backend of another OS is an error, an unusable named one fails closed |
| `sandbox.network` | `allow` (default, also in Custom) or `block`; projects may only set `block` |
| `sandbox.writable_paths` | At most 32 folders, absolute or `~/...`, 1–1,024 bytes, no `..`; projects may only remove entries |

`prompt_file` supplements the built-in compaction guidance; it does not replace
the compaction algorithm. It resolves within the directory of the configuration
that declares it. Explicit run overrides resolve prompt references within the
user configuration directory. Built-in prompts are in
`crates/core/builtins/prompts/`. The shared adaptive pipeline first performs reversible maintenance of old public tool intent, completed invocation arguments and redundant recovery copies. Delivered ordinary results remain available until semantic summarization consumes them; completion receipts retain pre-offload provenance. Maintenance checks for net savings before publishing a snapshot. It summarizes only under working-context pressure, after accounting for schemas, the actual output allowance and a safety margin. Archive size and reaching a tool-turn milestone do not themselves trigger summarization.

A fitting packet needs one summary call. When complete eligible records cannot fit, the runtime splits originals into ordered, UTF-8-safe source fragments, summarizes them sequentially, then consolidates bounded notes once. It never silently substitutes sampled head/tail text for complete processing. All stages must finish and pass the required ordered, nonempty state-section checks before a checkpoint is installed. This validates structure, not factual correctness. Stage notes are lossy even with complete processing coverage. Completed stages persist under the evidence directory's `summary-jobs/` and can be reused internally for matching source, prompt, model and output policy; automatic request retries do not restart completed stages. Ordered short records share bounded source packets rather than creating one request per message. A configured stage-count or packet-size overflow suspends the run before it can install incomplete state.

Each failed stage automatically retries up to twice for transient transport/provider errors, request timeout, incomplete/truncated output, empty text or a malformed required checkpoint. Healthy stages use one call. Every attempt must pass structural validation. Permanent provider errors, storage failures and impossible budget/stage plans fail immediately. Output limits are base, twice base, twice base; preparation reserves the maximum before dispatch. Retry-After remains a floor with exponential backoff and jitter for every retry. Summary HTTP 429 responses use `rate_limit_backoff_seconds` (default ten then twenty seconds); other transient summary errors use one then two seconds. This also covers upstream-unavailable errors returned with status 429; it does not identify their underlying cause or guarantee provider recovery. The absolute job/stage deadlines include requests and waits: three attempts are a maximum, and the final attempt may have less time. Completed stages and tools are never replayed as part of a request retry. The default overall job deadline is fifteen minutes (900 seconds), configurable up to thirty minutes (1800), while each request remains limited to five minutes (300). Each uncached attachment receives its own job deadline, capped by the separate `document_batch_timeout_seconds` deadline (default one hour). A document deadline spans all of its stages and retries; it is never reset per stage. Explicit existing timeout and retry overrides are preserved. No new summarization calls are made after success. Execution-only changes to job/request/batch timeouts, retry count or rate-limit wait do not invalidate completed stages or capsules. Cache identity still binds the resolved prompt, source, routed model, effective input/output budgets and pipeline. Exact pre-split cache identities can be migrated without provider calls when the current settings reconstruct the old key and no custom resolved prompt is active; the runtime does not scan for or guess unknown historical settings. Migration validates original identities and bounded cached stages before publishing a new immutable key.

`summary_max_tokens` caps the initial allowance, not every attempt. Each permitted retry currently uses up to twice that effective initial allowance, including retries after HTTP 429; increasing output does not itself resolve rate limiting. Provider output limits and the reserved input/output capacity can clamp these allowances further. The checkpoint allowance also depends on the usable message budget, system/current-request/recent-message space and continuation reserves. Increasing the ceiling alone may therefore leave small-budget requests unchanged. A larger effective initial allowance can avoid output-truncation retries, but requires room for both the maximum retry output and continued ordinary work. It cannot restore evidence excluded from summary input. Validated bounded original-derived archive excerpts now enter both summary paths independently of optional read caches; see [context limits and recovery](context-and-attachments.md).

Failure or cancellation keeps the working context unchanged, emits no fallback checkpoint and stops the current run. Before summarization, the runtime saves original evidence and exact job inputs internally. Failure messages report whether originals were saved; a storage failure does not imply durable recovery. After a noncancelled checkpoint failure with both originals and pending inputs saved, a bounded historical progress record is persisted before the terminal failure. It carries selected tool observations and original-source coordinates into a later ordinary request, including after restart, without restoring or executing the pending job. Its byte ceiling is one sixth of usable message tokens, clamped to 1,024–8,192 bytes; action receipts take priority over retrieval excerpts. It is not a summary, proof of tool success or current file state, or a complete action log. Stop and document-preparation failures do not publish this record. Current user instructions retain precedence. Metadata diagnostics distinguish `originals_saved` and `pending_job_saved` and record retry reason, attempt, backoff and remaining time without content; `suspended_progress` records only publication and context-replacement flags. There is no user-facing compaction retry command and no automatic recovery-to-compaction continuation loop.

Internal summaries disable reasoning independently of the selected answer effort. Known off controls retain LongCat 2.5 with thinking disabled and GPT 5.6/6 Luna with reasoning effort none. Other models use `longcat-2.5-preview-free` with thinking disabled for summaries, through the existing Go credentials and session. This routing is independent of the selected ordinary model. Diagnostics identify the effective model without recording response text.

An explicit `compaction.summary_model` selects one of those verified nonthinking routes without changing ordinary work. It follows normal user/project/run precedence; null clears an earlier override. Unsupported model IDs fail configuration validation, and a model missing from the loaded catalog fails setup before document preparation or provider generation. Routing, capacity, output limits, request telemetry, stage fingerprints and capsule identity use the same resolved model. There is no automatic model switch after provider failure. Changing the model may require new summaries; existing model-specific stages, capsules and original evidence remain available. Suspended document-job keys bind the resolved model and source path so equally sized models or identical documents at different paths cannot overwrite each other's recovery inputs. Ambiguous legacy pending inputs remain preserved, including during capsule migration; they do not cause automatic replay or new model calls.

Ordinary answers retain the selected model and configured reasoning behavior. Effort validation loads the Go catalog on first use after restart, before deciding whether an effort is supported. Catalog-loading failures remain explicit loading errors; Stop can cancel that fetch during run setup.
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

## Composer permissions

The shield offers only **Custom** (default) or **YOLO**, persisted per thread.
It has no configuration editor or advanced settings link.
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
Example allow rule: `{ "tool": "shell", "command": ["cargo", "test"], "action": "allow" }`.
Allow and ask rules match the program token exactly (`/bin/echo` or a project-local
`./echo` does not match `["echo"]`, so an allow rule never widens). Deny rules also
match the program by basename, lowercased on macOS/Windows and without
`.exe`/`.cmd`/`.bat` on Windows, so `["rm"]` denies `rm`, `/bin/rm`, `./rm` and `RM`.
Wrappers (`env`, `sudo`, `command`, `busybox`, `xargs`, `bash -c`) still bypass
deny rules: this is a tool policy, not a sandbox; deny the wrappers you do not want.

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
or the same server after its plugin revision changes, asks again.

**Chat grants.** The approval dialog offers **Allow once**, **Allow in this chat** and
**Deny**. *Allow in this chat* runs the action now and saves the grant to the chat's own
file, `chat_grants/<chat id>.json` in the app data directory (beside `threads.json`). Format:
`{"version":1,"grants":[{"tool":"shell","command":["cargo","test"],"risk":"execute"}, {"tool":"mcp","server":"<16 hex>","risk":"network"}]}`;
`command` is the covered argv prefix (`shell`, `git` and `shell_unsandboxed` only), `server` marks a whole-MCP-server
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
also escalates destructive `checkout`, `branch` and `commit` calls), so a rule with
`"risk": "execute"` does not cover them. So a rule `["git","push"]` allows
`git push origin main` but `git push --force` falls through to later rules and the
default (ask; denied when unattended). A rule such as `["git","push","--force"]` is
honoured. A broad allow rule without `command` (`{"tool":"shell","action":"allow"}`, or
`"tool":"*"`) is trust in the tool but not in its destructive variants; add
`"risk": "destructive"` to such a rule if you really mean them. Wrappers (`sudo rm -rf`,
`bash -c "git push -f"`) are not recognized as variants.

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
always see the argv the model sent.

Unattended CLI `ask` requests deny; use explicit policy or an interactive client.

YOLO means no approval prompts: it bypasses harness approval gates, filesystem
root restrictions, skill tool filters, and the Git subcommand allowlist. The OS
sandbox (and the file guard) still apply, as do operating-system permissions,
tool input validation, time/output bounds, and child environment handling; a
command the sandbox blocks fails without asking. Custom is a tool policy on top
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
file path). Settings → Permissions renders it without any edit control, and
`themis permissions [PROJECT] [--thread ID] [--json]` prints the same view (the
current directory when neither is given). `get_runtime_configuration` also
returns `rule_sources` beside the resolved configuration.

## Diagnostics

Logs stay out of the desktop UI. Advanced users can query structured records
through the CLI/API or inspect the local JSONL files. Filters accept exact
severity, service, or operation ID.

Records contain epoch-millisecond timestamp, severity, service, event, and
metadata. Events cover server lifecycle, configuration saves, integration
inspection/import/test outcomes, approval decisions, the sandbox state each run
captured (`sandbox`/`run_plan`: state, backend and network, never paths or
commands), and compaction stages.
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

The resolver and read/query APIs are available through the actual CLI:

```sh
themis call get_runtime_configuration '{"projectRoot":"/path/to/project"}'
themis call query_diagnostic_logs '{"service":"integration","limit":200}'
themis call query_diagnostic_logs '{"operationId":"OPERATION_UUID"}'
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

The app starts enabled global and selected-project MCP connections asynchronously on entry. Enabling a connection authorizes initialization and tool discovery; imported connections still start disabled. Consent is bound to the exact definition: Themis keeps a fingerprint (command, args, url, environment references, bearer variable, and the plugin's resource files and their executable flags, bounded to 256 files / 8 MiB, over which consent fails closed) of what a person approved in `mcp_consent.json` beside the plugin registry. A person saving or enabling the connection in the UI or CLI records it; so does answering the dialog for an agent's change, and only while the stored definition still matches what that dialog showed. Agent management calls on one plugin run one at a time. An agent change applied under YOLO or an allow rule takes effect but records no consent: the connection shows "Needs your approval" until you enable it. When the sole executable effect of an agent's save is one command-based connection (no hooks or skills), the single argv dialog replaces the generic one; otherwise both appear. Chat grants for an MCP server also expire with its definition (the definition is part of the server id). A definition that is new or changed by anything else (a repository's `.themis/plugins/` or `.themis/config.jsonc`, a catalog update or import, an unapproved edit) shows "Needs your approval" and does not start, in the background or in a run, in Custom or YOLO, until a person enables it again; imports still arrive disabled. On the first start after upgrading, already-enabled global definitions are seeded as approved; project definitions never are. An agent that creates, edits or enables a command-based connection through `manage_integrations` is asked about the exact command (shown as structured argv, one connection per call) every time; that dialog offers only Allow once and Deny and is never remembered for the chat, YOLO skips it as the user's explicit trust, and a deny rule refuses it. Runs no longer show a "Connect MCP server" prompt: explicit user/project deny rules for `mcp_start_*` (or the server) still prevent startup, in the background and in a run, and YOLO ignores rules as before. Tool calls are separate approvals; one **Allow in this chat** covers the server's other tools. Calls from a chat continue to use that run's captured approval policy and approval mode, including when they reuse an existing transport. Startup has a 30-second deadline per server and failures are isolated. Definitions remain in the existing integration registry; transports and observed status are ephemeral shared-server state. Changing or disabling a definition releases its idle connection while active runs retain their immutable snapshot.

The shared `plugin_action` API accepts `mcp_status` with `projectRoot`. `connect: true` reconciles enabled definitions and starts missing connections without waiting; `retry: true` also retries failed connections. Without those flags it only reads observed status. This supports CLI verification through `themis call plugin_action`. App entry, project selection and server reconnection trigger initialization automatically.

Agent delivery limits (16 KiB per result, 48 KiB per batch, 64 KiB per serialized tool call, each reduced for small context budgets) are runtime safeguards rather than new configuration fields. The context estimate includes tool schemas and an answer reserve. Reads default to 4,000 content characters, and repeated archive recovery is bounded within each run. See [context limits and recovery](context-and-attachments.md) for offloading, errors, and the distinction between model delivery and source/tool capture limits.

## Context accounting API

`logging.context_metrics` is captured once per run. `off` disables optional request accounting; bounds, evidence saves and loop guards remain active. `summary` retains aggregate counts and the latest request; `detailed` also retains the last 32 request records with mutually exclusive role and purpose distributions. These are alternative views of the same message bytes, never additive. Estimates use serialized bytes and message overhead, not a verified model tokenizer. Within a run, budget decisions scale the estimate by the ratio of provider-reported to estimated input tokens (clamped 0.75-4.0, no change until a request reports usage) and a context-length-exceeded error triggers one compaction and one retry; `token_calibration` samples in the snapshot show estimate versus report per request. No new configuration fields; see [estimate calibration](context-and-attachments.md#estimate-calibration-and-overflow-recovery). The live catalog is joined to models.dev context/output limits. Ordinary requests clamp to the selected model's published capacity; internal summaries separately clamp to the effective nonthinking summary model. Unknown production capacity uses a conservative 24k admission limit and is labeled unknown. Published metadata is not proof of successful live requests at that capacity.

Every actual provider attempt, including internal summaries and retries, contributes to request counts and latency. Reported input/output totals remain separate from cached-input and reasoning-output subcounts, which are included in normalized parent counts. Anthropic uncached input, cache reads and cache creation are combined once; OpenAI cache detail is already included in input. Missing usage is null/unavailable, with missing-request counts; it is never reported as zero. Partial streams retain reported usage when available, and cancelled/failed requests remain in accounting. Streaming Chat Completions requests ask for usage and consume trailing usage frames. Token cost is unavailable unless a verified price source is supplied.

Metadata snapshots persist as context_usage events in the existing sessions database. The final accounting snapshot can follow the terminal event so cancelled request cleanup is included. Query without reading conversation payloads:

```sh
themis call get_context_usage '{"threadId":"THREAD","runId":"RUN"}'
```

Omit runId for the latest snapshot; no available snapshot returns null. Diagnostic JSONL remains bounded and rotating as documented below. It records correlation IDs, pipeline/job/stage/attempt, model, sizes, estimates, provider usage, timing and outcome categories. It excludes prompts, filenames, arguments, results, document passages, provider bodies and credentials. Historical source excerpts and stage notes belong only in transcript/evidence storage. This provides backend data for a future dashboard; no raw-log viewer or new dashboard UI is added. Usage and runtime events are observations, not automatic judgments of narrative or coding correctness.

## Shared-server event stream additions

Besides thread, approval and review events, the server broadcasts `thread-changed` (`{kind: created|updated|removed, project_root, thread_id, thread?}`) after a successful `create_thread`, `rename_thread`, `set_thread_approval_mode`, `set_thread_effort`, `set_thread_skills` or `discard_thread`, so an open desktop window follows changes made by `themis call` or another client. It carries metadata only. The `approval-request` event carries `grant_scope` (`{argv, covers_further_args}`, shell only): the raw command prefix an "allow in this chat" grant covers, matching the stored grant key. It also carries `grantable` (`false` for destructive actions, which only offer Allow once and Deny) and the `approve_action` decision is `once`, `chat` (formerly `always`, still accepted) or `deny`. When a dialog times out the server emits `approval-expired` (`{thread_id, approval_id}`) after denying it, so clients remove the stale dialog. Streamed assistant text is persisted before a run's terminal event, including Stopped and failed runs.
