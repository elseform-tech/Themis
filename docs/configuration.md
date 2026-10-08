# Configuration, scope, permissions, and diagnostics

Desktop and CLI use the same local app server. Settings → Configuration edits
runtime JSONC and shows the effective values, source of each field, and schema.
Invalid configuration remains visible for repair; a run does not silently ignore it.

## Files and scope

The production macOS app-data directory is
`~/Library/Application Support/ai.themis.desktop/`. Linux uses
`${XDG_DATA_HOME:-~/.local/share}/ai.themis.desktop/`; Windows uses
`%APPDATA%/ai.themis.desktop/`. CLI `--data-dir DIR` selects an isolated store.
QA builds with a different app identifier have separate data directories.

| Location | Purpose | Scope |
| --- | --- | --- |
| App-data `runtime.jsonc` | Runtime defaults, Custom approval rules, logging, discovery | User, across projects |
| Project `.themis/config.jsonc` | Project runtime overrides and additional approval restrictions | Selected project/run root |
| App-data `settings.json` | Existing desktop preferences and legacy run defaults | User |
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

Use Integrations' **Installation scope** selector to choose **User (Global)** or
**Project**. Selecting a project does not force a global import into that project.
Global packages are available across projects; local packages retain their own
scope identity. Use shared import/edit/export APIs rather than hand-editing the
revisioned registry. Portable plugin manifests and MCP JSON are import sources,
not a second configuration store. See [plugins](plugins.md) for formats.

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
  "compaction": {
    "timeout_seconds": 240,
    "retry_limit": 1,
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
  }
}
```

`skills.paths` replaces project skill search roots; it does not change conventional
user discovery. Skill and instruction paths must be relative, without parent
traversal, and remain inside the project after canonicalization. Each list permits
at most 32 paths. Instructions are UTF-8 project guidance, limited to 64 KiB
combined, and do not grant permissions. There is no implicit instruction globbing.

## Supported limits

| Field | Range/default |
| --- | --- |
| `context_token_budget` | 2,000–200,000; omitted uses Settings |
| `recent_messages` | 2–200; omitted uses Settings |
| `total_turns` | 1–2,000; omitted uses Settings |
| `logging.level` | `debug`, `info` (default), `warn`, `error` |
| `compaction.timeout_seconds` | 1–240; default 240 for the shared summarization deadline |
| `compaction.retry_limit` | 0 or 1; default 1 |
| `compaction.section_max_tokens` | 256–16,384; default 4,096 |
| `compaction.summary_max_tokens` | 256–16,384; default 8,192 |
| `compaction.prompt_file` | Optional UTF-8 Markdown, at most 32 KiB |
| `approval.rules` | At most 256 ordered rules |

`prompt_file` supplements the built-in compaction guidance; it does not replace
the compaction algorithm. It resolves within the directory of the configuration
that declares it. Explicit run overrides resolve prompt references within the
user configuration directory. Built-in prompts are in
`crates/core/builtins/prompts/`. Section requests, reconciliation, and retries
share the summarization deadline; recovery checkpoint writes can add time
outside that deadline. Original evidence
and recoverable checkpoints remain separate from the model's summary; this
configuration change does not add compaction UI or guarantee lossless recall.

## Composer permissions

The shield selects **Custom** (default) or **YOLO**, persisted per thread.
Custom checks rules in order: first matching tool/risk wins, otherwise `default`
applies. Tool names match exactly or with a trailing `*` prefix wildcard.
Actions are `allow`, `ask`, or `deny`; risks are `read`, `write`, `execute`,
`network`, and `destructive`. Built-in policy allows reads and asks for other
actions. The existing confirm-reads setting still applies when no explicit
approval configuration is supplied.

Project approval configuration may add deny rules only. Those restrictions are
evaluated ahead of user rules; a project cannot grant itself access. Explicit
run overrides are caller-supplied policy. An interactive **Always** grant is
cached for the same tool and risk within the run and cannot override a deny rule.
Unattended CLI `ask` requests deny; use explicit policy or an interactive client.

YOLO bypasses harness approval gates, filesystem root restrictions, skill tool
filters, and the Git subcommand allowlist. Operating-system permissions, tool
input validation, time/output bounds, and child environment handling remain.
Custom is a tool policy, not an OS sandbox: approved programs, including an
explicit shell interpreter, can perform their own filesystem/network operations.
See [sandbox](sandbox.md) for the enforcement boundary.

## Diagnostics

Settings → Diagnostics reads, filters, copies, and exports JSONL records. Filter
by exact severity, service, or operation ID. Integration errors provide a
**View diagnostics** link to the associated operation.

Records contain epoch-millisecond timestamp, severity, service, event, and
metadata. Events cover server lifecycle, configuration saves, integration
inspection/import/test outcomes, approval decisions, and compaction stages.
Request bodies, prompts, tool arguments/results, and provider bodies are excluded.
Known credential fields and referenced secret environment values are redacted;
debug subprocess stderr is bounded and filtered. Debug output from third-party
programs may contain other private information; review exports before sharing.

Three files retain at most 2 MiB each. Individual records are capped at 16 KiB;
queries default to the latest 200 records and support up to 2,000. Rotation is
local retention, not a permanent audit archive. Severity thresholds can suppress
lower-severity records, including info-level approval events. Export preserves
the currently filtered records. Connection **Ready** is a successful explicit
test for that saved definition in the current UI session, not continuous health
monitoring or a promise that a future connection will succeed.

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
the composer, Integrations, configuration editor, and bridge.

Automated tests prove their exercised paths. Native verification, third-party
network acceptance, and model retention benchmarks are separate evidence;
previous benchmark results are dated reports, not automatic runtime audits.
