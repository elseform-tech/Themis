# Themis tool permissions and isolation

The composer shield selects **Custom** (default) or **YOLO**. This is a harness
policy, not an operating-system sandbox. See [configuration](configuration.md)
for JSONC examples and exact limits.

## Custom mode

Core file tools confine paths to the canonical project/worktree root and reject
traversal and symlink escapes. The tools are `list_dir`, `read_file`, `write_file`,
`copy_file`, `move_file`, `delete_file`, `create_dir`, `search_file`, `shell`,
`apply_patch`, and `git`. Plugins can supply MCP tools and integration management
adds its own tools.

Actions have a risk (`read`, `write`, `execute`, `network`, `destructive`). Ordered
Custom rules choose allow, ask, or deny; the first matching rule wins. Default
policy allows reads and asks for other actions. Rules may carry an optional `command`
argv prefix to scope `shell` rules (for example allow `["cargo","test"]`). Project rules may only add deny
restrictions. Explicit user/run allow rules proceed without the interactive gate.

An ask invokes the desktop/CLI approval hook. **Once** grants one action,
**Always** remembers the grant key for this run: the tool (and risk) for most tools;
for `shell`, program plus subcommand only for a short list of subcommand-style CLIs
such as `cargo test`, and the exact full argv for everything else, and **Deny** rejects it.
Cached grants cannot bypass policy denies. Approval timeouts (default five minutes),
lost clients, and unattended CLI requests deny. The interactive hook denies
non-read actions in non-Git projects; explicit allow policy or YOLO bypasses
that interactive restriction. A non-Git folder is not an OS-enforced read-only
sandbox.

Custom's Git tool limits subcommands; permitted commands still consult approval
policy, including reads. Skill tool allowlists further filter available tools.
Hooks and MCP initialization instructions do not grant permissions.

## YOLO mode

YOLO bypasses harness approval, the core file-tool root restriction, the Git
subcommand allowlist, and skill tool filtering. OS permissions and tool input
validation, timeouts, output bounds, and child environment handling still apply.
The choice is saved per thread. An active run retains its captured mode and
configuration; edits affect subsequent runs.

## Programs and filesystem access

The `shell` tool launches a program with an argv array, without implicitly
parsing shell strings. Pipes and redirects are literal arguments unless an
explicit interpreter such as `bash -c` is launched. An approved program can
perform its own filesystem/network operations: pinning its working directory
does not confine the process. Core path guards do not sandbox subprocesses or
third-party MCP servers/hooks.

Custom requires the tool working directory to stay inside the project root.
YOLO permits external paths. Command rules are a tool policy, not a sandbox: deny rules match the program basename
(case-insensitively on macOS/Windows) but wrappers such as `env`, `sudo` or `bash -c`
bypass them. The shell child runs in its own process group, so Ctrl-C in the CLI does
not signal it. The timeout kills the group, and **stopping a run kills the
blocking shell command it is waiting for** (whole process group, same
SIGTERM-then-SIGKILL sequence); the call fails with "command was stopped by you
and killed". Background tasks (below) are the exception: they outlive the turn.
After the direct child exits, output is collected for at most 1.5 seconds. A
backgrounded grandchild (`sh -c 'server &'`) inherits the output pipes and no longer
hangs the call; the result then ends with a note saying the process still holds
its output and suggesting `background=true`. That grandchild keeps running.
Shell output is bounded at 32 KiB; the default
execution timeout is 120 seconds. On timeout the whole process tree is killed:
on Unix the child leads its own process group, which receives SIGTERM, a short
grace period, then SIGKILL; on Windows `taskkill /T /F` ends the tree (a
descendant that detaches from its parent is not reached). Child environment
handling excludes provider credentials from normal tool launches. Children
receive only `PATH`, `TMPDIR`/`TEMP`/`TMP`, `LANG`, `LC_ALL` and other `LC_*`,
`TERM`, plus `HOME`/`USER`/`LOGNAME`/`SSH_AUTH_SOCK` on Unix or the Windows
system/user variables; any name containing `KEY`, `TOKEN` or `SECRET` is
dropped even if otherwise allowed. MCP servers can receive explicitly configured environment-variable references.
Hooks receive the fixed allowlist plus `CLAUDE_PLUGIN_ROOT`, rather than a
configurable environment map. Imported MCP servers and hooks
begin disabled and require explicit activation; hook trust remains separate
from installation.

## Shared checkout and secrets

Threads currently share the project checkout, including existing uncommitted
files. File edits affect that checkout directly. Select a separate Git worktree
as a project when you want checkout isolation. Approved programs and YOLO can
access other paths. Inspect changes in your Git client. Stopping or removing a
thread does not undo its completed filesystem operations.

Entered API keys remain in macOS Keychain; launch environment keys are handled
by the backend. Only key availability crosses the settings bridge. Diagnostic
records exclude prompt/tool/provider bodies, redact credential fields and
referenced secret values, and bound debug stderr. Third-party stderr may contain
other private information; inspect exported logs before sharing them.

## On an unexpected action

1. Deny a pending request or stop the run.
2. Inspect the worktree and any paths affected by approved programs.
3. Review the recorded tool events; advanced users can inspect local diagnostic logs.
4. If a key was pasted into a prompt/file or otherwise exposed, rotate it with
   the provider and replace the stored key.

## Background shell tasks

`shell` with `"background": true` starts the program and returns at once with a
`task_id` (optional `background_timeout_secs`, default 7,200, maximum 86,400). The
call goes through exactly the same approval path as a blocking call: same
`ToolAction` argv, same grant key and command-scoped `approval.rules[].command`
matching, same Custom/YOLO behavior; the approval summary adds "in the
background". The program gets the same scrubbed environment, no stdin and its
own process group. Output (stdout and stderr interleaved) feeds a rolling log of
the last 64 KiB, captured on helper threads, cut only at UTF-8 character
boundaries.

The separate **`background_task`** tool (a separate tool keeps the `shell`
schema simple and lets a skill grant managing tasks together with `shell`)
takes `action`:

| Action | Result |
| --- | --- |
| `list` | this chat's tasks (id, state, exit code, command, log size) |
| `status` | state, exit code and the last 20 output lines |
| `log` | a page of at most 16 KiB (default 8 KiB): `offset` (use the previous `next_offset`; omit for the newest output), `next_offset`, `total_bytes`, `oldest_retained_offset`, `more` |
| `stop` | kills the whole process group (SIGTERM, 300 ms grace, SIGKILL) and waits up to 5 s |

`background_task` needs no extra approval: it reaches only tasks this chat
already started with an approved `shell` call, and returns bounded output.
Tasks are private to their chat.

Limits: 4 running tasks per chat and 16 per app server (starting another fails
with the limit named); the last 20 finished records per chat are kept.

Lifetime. A task belongs to its chat and **outlives the turn**; ending or
stopping a run does not stop it. It is killed (whole group) when the chat is
deleted (also when its project is deleted), when the app server shuts down (the
`shutdown` request, SIGTERM or SIGINT) and at its deadline. When the main
process exits on its own, the rest of its process group is stopped so nothing is
orphaned; a program that daemonizes into a new session escapes this, as it does
for blocking commands. Records and finished logs are persisted under
`background/` in the app data directory. If the server dies without cleanup
(crash, SIGKILL, power loss), tasks that were running are reported as **lost**
after the next start, never as running; Themis does not track or kill such
processes, so check the port or `pgrep` if you expect one to be left.
Graceful shutdown records tasks as stopped and keeps their logs.

A started task is recorded in the task ledger like any other action (the start
result carries the `task_id`), so the agent still knows it after context
compaction; `background_task list` recovers it from the app server as well.

## Automation check commands

A watch's `check_command` (see [automations](automations.md)) is a scheduled
`shell` action and follows this policy: the same argv, Execute risk, command-scoped
and deny rules. Creating or updating an automation is approval-free for the agent's
own chat; executing a program is not. In Custom mode an `ask` becomes an approval of
the exact argv in the creating run, asked as a `shell` action (command-scoped
rules, the non-git refusal, no grant cache) using the mode and policy that run captured,
stored with the automation and invalidated by any change; the scheduler never prompts and pauses a watch that lacks approval or whose
policy now denies. Policy is re-evaluated before every execution, so a later deny
rule or a switch from YOLO to Custom stops the check. Run-scoped grants do not persist. Deny rules also still reject an agent's own-chat
automation lifecycle actions (`manage_automations*`); `ask` proceeds without a prompt.
