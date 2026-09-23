# Themis sandbox model

Plain-language threat model for running coding agents on your machine. This
document describes what this repo's code actually enforces — paths and names
refer to the real implementation.

## What agents can do

An agent acts through exactly 11 tools (`boxed_tools` in
`crates/core/src/tools.rs`): `list_dir`, `read_file`, `write_file`,
`copy_file`, `move_file`, `delete_file`, `create_dir`, `search_file`,
`shell`, `apply_patch`, `git`. Every tool call is classified with a risk
level (`read`, `write`, `execute`, `network`, `destructive`) and checked
against the approval hook before it runs.

Concretely, an approved agent can: read, write, copy, move, and delete files
**inside the project root**; create directories; search file contents; run
programs; apply patches; and run a limited git subset (read-only commands
freely; `add`, `commit`, `checkout`, `branch` with approval; everything else
is denied outright).

## What agents cannot do

- **Escape the project root.** All file paths resolve inside the canonicalized
  root; `..` tricks and absolute detours are rejected (`resolve_within_root`).
- **Run shell strings.** There is no shell: the `shell` tool takes a program
  plus an argv array, resolved via `PATH`. Pipes, redirects, glob expansion,
  and `VAR=x cmd` prefixes do not exist — a command containing `|` passes a
  literal pipe character as an argument.
- **Touch your checkout directly.** On git projects the agent works in a
  per-thread worktree; your files change only when you press **Merge**.
- **See your keys.** API keys live in backend session memory or the launch environment and only presence
  (`set`/`missing`) crosses the bridge to the UI. Diagnostics, settings
  snapshots, logs, and error strings never carry secret values.
- **Act silently on writes.** Every non-read tool call needs your decision
  (or a cached Always for that tool in that run). There is no "approve
  everything" mode.

## Approvals

Policy (`crates/desktop/src/approvals.rs` + `CachingApprovals` in
`crates/core/src/runtime.rs`):

- `read`-risk calls are auto-allowed without a dialog.
- Everything else emits an `approval-request` event and blocks for your
  decision in the dialog: **Once**, **Always** (remembered per tool name for
  the rest of the run only), or **Deny**.
- The dialog waits ~5 minutes (`APPROVAL_TIMEOUT`), then denies. Lost
  connections and expired requests also deny. The safe default is always "no".

## Read-only mode

Open a folder that is **not** a git checkout and the thread runs read-only:
reads proceed, every other action is denied outright without even a dialog.
Use this for exploring untrusted code. `git` detection happens at project
open; the sidebar tags such projects **no git**.

## Worktree isolation

Each thread on a git project gets a branch `themis/<first-8-of-thread-id>`
and a dedicated worktree (`crates/desktop/src/worktree.rs`). The agent's
edits, commits, and command side effects land there — never in your checkout.
**Merge** applies the worktree changes to your checkout only when they apply
cleanly; on conflict it reports the paths and touches nothing. **Discard**
deletes the thread, its worktree, and its branch.

## Shell policy

- **argv-only, no shell** (above). The working directory is pinned inside the
  project root (`cwd` is relative; empty means the root).
- **Always approval-gated** (`RiskLevel::Execute`): every invocation needs a
  dialog decision or a cached Always for `shell`.
- **Output is capped**: combined stdout/stderr is truncated at 32 KiB
  (`MAX_COMMAND_OUTPUT`) with a `[output truncated]` marker.
- **Execution timeout**: shell commands default to 120 seconds.
- **Environment scrubbing**: child processes receive an allowlisted environment;
  variable names containing KEY, TOKEN or SECRET are excluded. Go reads
  `OPENCODE_KEY` in the backend; entered keys remain in backend session memory.

Related real timeouts: provider HTTP calls time out after 120 s, and approval
dialogs deny after ~5 minutes.

## On a suspicious action

1. Press **Deny**. Denials are safe: the agent is told and must adapt or stop.
2. Find the worktree path in Thread options and inspect its changes in your editor or Git client.
3. If anything looks wrong, **Discard** the thread (worktree and branch are
   removed; your checkout was never touched).
4. If a key may have leaked (e.g. pasted into a prompt or file), rotate it at
   your provider, then replace it in Settings → Models & connections.
5. Use Settings → General → Updates & diagnostics → **Copy diagnostics** to capture a secret-free
   snapshot for a bug report.
