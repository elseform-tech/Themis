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
| App-data `runtime.jsonc` | Runtime defaults, Custom approval rules, logging, discovery | User, across projects |
| Project `.themis/config.jsonc` | Project runtime overrides and additional approval restrictions | Selected project/run root |
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

Credential reads run outside the async backend workers and wait at most 30
seconds, so a blocked read does not freeze other status/Stop requests. Stop
cancels a run's credential wait before skill/MCP setup. The native OS read may
finish later; no authentication requirement is bypassed. Credential availability
reports false when a read fails or times out; the value never reaches the UI.

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
  "compaction": {
    "timeout_seconds": 900,
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
| `context_token_budget` | 2,000–256,000; omitted uses Settings (default 200,000) |
| `recent_messages` | 2–200; omitted uses Settings |
| `total_turns` | 1–2,000; omitted uses Settings |
| `logging.level` | `debug`, `info` (default), `warn`, `error` |
| `compaction.timeout_seconds` | 1–1800; default 900 for the entire summary job, including retries and consolidation; explicit existing values are preserved |
| `logging.context_metrics` | `off`, `summary` (default), `detailed`; controls content-free accounting, never safety guards |
| `compaction.request_timeout_seconds` | 1–300; default 300; additionally limited by the remaining job/stage allocation |
| `compaction.maintenance_threshold_ratio` | 0.50–0.95; default 0.83 of usable message allowance |
| `compaction.semantic_threshold_ratio` | Above maintenance and at most 0.97; default 0.93 |
| `compaction.target_headroom_interactions` | 1–20; default 6; recent observed growth adjusts the trigger within a bounded reserve |
| `compaction.max_stages` | 1–128; default 32, including consolidation |
| `compaction.retry_limit` | 0–2; default 2 (up to three attempts per failed stage) |
| `compaction.section_max_tokens` | Legacy compatibility field, 256–16,384; retained for file compatibility; stage output is computed from consolidation capacity |
| `compaction.document_context_token_budget` | Internal full-document preparation input budget, 24,000–1,000,000; default 200,000; clamped to the routed summary model capacity, with conservative 24,000 when unknown. Does not change ordinary context capacity. |
| `compaction.summary_max_tokens` | 256–16,384; default 8,192 |
| `compaction.prompt_file` | Optional UTF-8 Markdown, at most 32 KiB |
| `approval.rules` | At most 256 ordered rules |

`prompt_file` supplements the built-in compaction guidance; it does not replace
the compaction algorithm. It resolves within the directory of the configuration
that declares it. Explicit run overrides resolve prompt references within the
user configuration directory. Built-in prompts are in
`crates/core/builtins/prompts/`. The shared adaptive pipeline first performs reversible maintenance of old completed tool observations. It summarizes only under working-context pressure, after accounting for schemas, the actual output allowance and a safety margin. Archive size and reaching a tool-turn milestone do not themselves trigger summarization.

A fitting packet needs one summary call. When complete eligible records cannot fit, the runtime splits originals into ordered, UTF-8-safe source fragments, summarizes them sequentially, then consolidates bounded notes once. It never silently substitutes sampled head/tail text for complete processing. All stages must finish and pass the required ordered, nonempty state-section checks before a checkpoint is installed. This validates structure, not factual correctness. Stage notes are lossy even with complete processing coverage. Completed stages persist under the evidence directory's `summary-jobs/` and can be reused internally for matching source, prompt, model and output policy; automatic request retries do not restart completed stages. Ordered short records share bounded source packets rather than creating one request per message. A configured stage-count or packet-size overflow suspends the run before it can install incomplete state.

Each failed stage automatically retries up to twice for transient transport/provider errors, request timeout, incomplete/truncated output, empty text or a malformed required checkpoint. Healthy stages use one call. Every attempt must pass structural validation. Permanent provider errors, storage failures and impossible budget/stage plans fail immediately. Output limits are base, twice base, twice base; preparation reserves the maximum before dispatch. Retry-After remains a floor with exponential backoff and jitter for every retry. The absolute job/stage deadlines include requests and waits: three attempts are a maximum, and the final attempt may have less time. Completed stages and tools are never replayed as part of a request retry. The default overall job deadline is fifteen minutes (900 seconds), configurable up to thirty minutes (1800), while each request remains limited to five minutes (300). Explicit existing timeout and retry overrides are preserved. No new summarization calls are made after success.

`summary_max_tokens` is a ceiling, not a guaranteed request limit. The checkpoint allowance also depends on the usable message budget, system/current-request/recent-message space and continuation reserves. Increasing the ceiling alone may therefore leave small-budget requests unchanged. A larger effective initial allowance can avoid output-truncation retries, but requires room for both the maximum retry output and continued ordinary work. It cannot restore evidence excluded from summary input. Validated bounded original-derived archive excerpts now enter both summary paths independently of optional read caches; see [context limits and recovery](context-and-attachments.md).

Failure or cancellation keeps the working context unchanged, emits no fallback checkpoint and stops the current run. Before summarization, the runtime saves original evidence and exact job inputs internally. Failure messages report whether originals were saved; a storage failure does not imply durable recovery. Metadata diagnostics distinguish `originals_saved` and `pending_job_saved` and record retry reason, attempt, backoff and remaining time without content. There is no user-facing compaction retry command and no automatic recovery-to-compaction continuation loop.

Internal summaries disable reasoning independently of the selected answer effort. Known off controls retain LongCat 2.5 with thinking disabled and GPT 5.6/6 Luna with reasoning effort none. Other models use `longcat-2.5-preview-free` with thinking disabled for summaries, through the existing Go credentials and session. This routing is independent of the selected ordinary model. Diagnostics identify the effective model without recording response text.

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

## Composer permissions

The shield offers only **Custom** (default) or **YOLO**, persisted per thread.
It has no configuration editor or advanced settings link.
Custom checks rules in order: first matching tool/risk wins, otherwise `default`
applies. Tool names match exactly or with a trailing `*` prefix wildcard.
Actions are `allow`, `ask`, or `deny`; risks are `read`, `write`, `execute`,
`network`, and `destructive`. Built-in policy allows reads and asks for other
actions. The existing confirm-reads setting still applies when no explicit
approval configuration is supplied.

Project approval configuration may add deny rules only. Those restrictions are
evaluated ahead of user rules; a project cannot grant itself access. Explicit
run overrides are caller-supplied policy. An interactive **Allow for this run** grant is
cached for the same tool and risk within the run and cannot override a deny rule.
Unattended CLI `ask` requests deny; use explicit policy or an interactive client.

YOLO bypasses harness approval gates, filesystem root restrictions, skill tool
filters, and the Git subcommand allowlist. Operating-system permissions, tool
input validation, time/output bounds, and child environment handling remain.
Custom is a tool policy, not an OS sandbox: approved programs, including an
explicit shell interpreter, can perform their own filesystem/network operations.
See [sandbox](sandbox.md) for the enforcement boundary.

## Diagnostics

Logs stay out of the desktop UI. Advanced users can query structured records
through the CLI/API or inspect the local JSONL files. Filters accept exact
severity, service, or operation ID.

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

The app starts enabled global and selected-project MCP connections asynchronously on entry. Enabling a connection authorizes initialization and tool discovery; imported connections still start disabled. Explicit user/project approval denials prevent background startup. Calls from a chat continue to use that run's captured approval policy and approval mode, including when they reuse an existing transport. Startup has a 30-second deadline per server and failures are isolated. Definitions remain in the existing integration registry; transports and observed status are ephemeral shared-server state. Changing or disabling a definition releases its idle connection while active runs retain their immutable snapshot.

The shared `plugin_action` API accepts `mcp_status` with `projectRoot`. `connect: true` reconciles enabled definitions and starts missing connections without waiting; `retry: true` also retries failed connections. Without those flags it only reads observed status. This supports CLI verification through `themis call plugin_action`. App entry, project selection and server reconnection trigger initialization automatically.

Agent delivery limits (16 KiB per result, 48 KiB per batch, 64 KiB per serialized tool call, each reduced for small context budgets) are runtime safeguards rather than new configuration fields. The context estimate includes tool schemas and an answer reserve. Reads default to 4,000 content characters, and repeated archive recovery is bounded within each run. See [context limits and recovery](context-and-attachments.md) for offloading, errors, and the distinction between model delivery and source/tool capture limits.

## Context accounting API

`logging.context_metrics` is captured once per run. `off` disables optional request accounting; bounds, evidence saves and loop guards remain active. `summary` retains aggregate counts and the latest request; `detailed` also retains the last 32 request records with mutually exclusive role and purpose distributions. These are alternative views of the same message bytes, never additive. Estimates use serialized bytes and message overhead, not a verified model tokenizer. The live catalog is joined to models.dev context/output limits. Ordinary requests clamp to the selected model's published capacity; internal summaries separately clamp to the effective nonthinking summary model. Unknown production capacity uses a conservative 24k admission limit and is labeled unknown. Published metadata is not proof of successful live requests at that capacity.

Every actual provider attempt, including internal summaries and retries, contributes to request counts and latency. Reported input/output totals remain separate from cached-input and reasoning-output subcounts, which are included in normalized parent counts. Anthropic uncached input, cache reads and cache creation are combined once; OpenAI cache detail is already included in input. Missing usage is null/unavailable, with missing-request counts; it is never reported as zero. Partial streams retain reported usage when available, and cancelled/failed requests remain in accounting. Streaming Chat Completions requests ask for usage and consume trailing usage frames. Token cost is unavailable unless a verified price source is supplied.

Metadata snapshots persist as context_usage events in the existing sessions database. The final accounting snapshot can follow the terminal event so cancelled request cleanup is included. Query without reading conversation payloads:

```sh
themis call get_context_usage '{"threadId":"THREAD","runId":"RUN"}'
```

Omit runId for the latest snapshot; no available snapshot returns null. Diagnostic JSONL remains bounded and rotating as documented below. It records correlation IDs, pipeline/job/stage/attempt, model, sizes, estimates, provider usage, timing and outcome categories. It excludes prompts, filenames, arguments, results, document passages, provider bodies and credentials. Historical source excerpts and stage notes belong only in transcript/evidence storage. This provides backend data for a future dashboard; no raw-log viewer or new dashboard UI is added. Usage and runtime events are observations, not automatic judgments of narrative or coding correctness.
