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
policy allows reads and asks for other actions. Project rules may only add deny
restrictions. Explicit user/run allow rules proceed without the interactive gate.

An ask invokes the desktop/CLI approval hook. **Once** grants one action,
**Always** remembers the same tool and risk for this run, and **Deny** rejects it.
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
YOLO permits external paths. Shell output is bounded at 32 KiB; the default
execution timeout is 120 seconds. Child environment handling excludes provider
credentials from normal tool launches. MCP servers can receive explicitly configured environment-variable references.
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
