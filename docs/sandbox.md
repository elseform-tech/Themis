# Themis tool permissions and the OS sandbox

Two layers decide what an agent can do. The **approval policy** (composer shield:
**Custom**, the default, or **YOLO**) decides which tool calls run and which ask
you first. The **operating-system sandbox** confines the programs those calls
start. In Custom it is the full sandbox described below. In YOLO it keeps only
one wall: nothing the agent runs can write Themis' app data or the project's
`.themis/` folder, so it cannot grant itself permissions; everything else is
open (see [YOLO mode](#yolo-mode)). See [configuration](configuration.md) for
JSONC examples and exact limits.

## The OS sandbox

### What it covers

| Covered | Not covered |
| --- | --- |
| `shell` commands (blocking calls) | Command-based MCP servers |
| Background tasks (`shell` with `background: true`) | Plugin hooks |
| The `git` tool | Themis' own in-process work (see the file guard below) |
| Automation watch `check_command`s | Programs you start outside Themis |

The `git` tool is covered because git runs repository hooks and configured
helpers, which would otherwise be an unsandboxed way to run code. MCP servers
and hooks are long-lived integrations you enable yourself (with consent bound to
their exact definition); they run with your permissions as before.

### Backends

| System | `backend: "auto"` picks | Explicit choices |
| --- | --- | --- |
| macOS | Seatbelt: `/usr/bin/sandbox-exec` with a generated profile | `seatbelt` |
| Linux | bubblewrap when `bwrap` is installed and can create a sandbox (user namespaces allowed), otherwise Landlock (Linux 5.13 or later with Landlock enabled) | `bubblewrap`, `landlock` |
| Windows | none yet | none |

`auto` never blocks a command: when nothing is usable, commands run unsandboxed
exactly as before and the status says "not available on this system". A backend
you name explicitly that cannot be used on this machine **fails closed**: every
covered command is refused with an error naming the backend until you install
it or switch back to `auto`. Naming a backend that does not exist on this
operating system at all (for example `bubblewrap` on macOS) is a configuration
error. Each backend is probed once per app-server process.

### What a sandboxed command may do

- **Write** only to the run's project (or worktree) root, temporary folders
  (`$TMPDIR` and, on macOS, its sibling per-user cache folder, `/tmp`,
  `/var/tmp`, and `/dev/shm` on Linux) and `sandbox.writable_paths`.
- **Never write** to the Themis app-data folder (`runtime.jsonc`,
  `chat_grants/`, `queues/`, `goals/`, `mcp_consent.json`, `plugins/registry.json`,
  `sessions.sqlite3`, ...) or to the project's `.themis/` folder, even inside a
  writable folder, so an agent cannot grant itself permissions.
- **Read** everything else except these secret locations under your home folder,
  which can neither be read nor written: `~/.ssh`, `~/.gnupg`, `~/.aws`,
  `~/.azure`, `~/.config/gcloud`, `~/.kube`, `~/.netrc`, `~/.git-credentials`,
  `~/.config/gh`, `~/.password-store`, `~/.local/share/keyrings`,
  `~/.cargo/credentials`, `~/.cargo/credentials.toml`, `~/.pypirc`,
  `~/.gem/credentials`, `~/.vault-token`, `~/.terraform.d/credentials.tfrc.json`
  and, on macOS, `~/Library/Keychains`. Tools that need them (an SSH
  `git push`, `gh`, `kubectl`, `aws`) therefore fail inside the sandbox.
- **Keychain** (macOS): programs reach saved passwords through the Keychain's
  services, not by reading its files, so Seatbelt also blocks those services
  (`com.apple.SecurityServer`, `com.apple.securityd`, `com.apple.securityd.xpc`).
  An agent's command can neither read saved passwords nor raise Keychain
  prompts (headless Chrome, for example, otherwise asks for "Chrome Safe
  Storage"). HTTPS certificate checks use other services and keep working.
  Tools that use the Keychain, such as git's HTTPS credential helper, fail;
  Themis tells the agent so. `"allow_keychain": true` in the user
  `runtime.jsonc` lets them use it again.
- **Network**: allowed by default (always in YOLO). `"network": "block"` blocks
  all IP traffic, including `localhost` (Seatbelt: outbound, inbound and bind;
  bubblewrap: a private network namespace; Landlock: TCP bind/connect only, and
  only with Landlock ABI 4, Linux 6.7, or later).

Tools that keep caches or locks in your home folder (`cargo` in `~/.cargo`,
`npm` in `~/.npm`, `pip` in `~/.cache/pip`) need those folders in
`writable_paths`, for example `"writable_paths": ["~/.cargo", "~/.npm"]`.
Writable paths are folders, absolute or starting with `~/`; at most 32, each at
most 1,024 bytes, without `..`. They never override the app-data, `.themis/` or
secret protections.

### Configuration

The sandbox is configured only in files, never in the app. In the user
`runtime.jsonc`:

```jsonc
"sandbox": {
  "enabled": true,          // default
  "backend": "auto",        // auto | seatbelt | bubblewrap | landlock
  "network": "allow",       // allow | block
  "writable_paths": [],     // absolute or ~/ folders
  "allow_keychain": false   // macOS: let commands use the Keychain
}
```

A project's `.themis/config.jsonc` (and an explicit run override) may only
tighten: set `"network": "block"`, set `"enabled": true`, repeat the user's
`backend`, list a subset of the user's `writable_paths`, or set
`"allow_keychain": false`. Disabling the sandbox, changing the backend,
allowing a network the user blocks, adding a writable path or allowing the
Keychain is a validation error, and runs are refused until it is fixed.
Like the rest of the configuration, the sandbox is captured once when a run
starts.

### When the sandbox blocks a command

No backend reports denials to Themis directly (Seatbelt logs them to the system
log; bubblewrap and Landlock just return an error to the program), so Themis
reads the failed command's own output. It treats a failure as a sandbox block
only when:

- the command failed (non-zero exit), and
- a line carries a permission error (`Operation not permitted`, `Permission
  denied`, `Read-only file system`) and names a path the sandbox blocks: a secret
  location, the app data or `.themis/` folder, or a folder outside the writable
  ones that the operating system itself would let you write; or a path under a
  secret location is reported missing (bubblewrap hides those folders); or
- the network is blocked and a line carries a typical network failure
  (`Could not resolve host`, `Couldn't connect to server`, `Network is
  unreachable`, ...).

Then, in **Custom**, Themis asks **Run outside the sandbox?** with the usual
**Allow once (1)** · **Allow in this chat (2)** · **Deny (3)** and the reason.
Allowing reruns the same argv in the same folder without the sandbox. The
request is the tool `shell_unsandboxed` with the command's argv, so rules and
chat grants for it are keyed like other shell commands but kept apart from
`shell`: allowing `cargo test` does not allow running it unsandboxed, and a
`{"tool":"shell_unsandboxed","action":"deny"}` rule refuses every escape.
Leaving the sandbox always needs a person: your answer in this dialog, a
**Allow in this chat** grant you made for it, or a rule you wrote in your user
`runtime.jsonc` whose `tool` is exactly `shell_unsandboxed`. Wildcard (`*`),
prefix (`shell*`) and risk-only allow rules never cover it, an `allow` default
still asks, and deny rules of any shape (including `*`) refuse it without
asking. The request is at least `execute` risk even when the original call was a
read (such as `git status`), so the read shortcuts never pass it. Destructive
commands stay **Allow once** / **Deny** only. Automation runs never ask for it
(only an existing chat grant lets one out), and non-Git projects refuse it. In **YOLO** nothing is
asked: the failure goes back to the agent with a short note that the sandbox
blocked it. Watch checks are never escalated (the scheduler asks nobody), and
background tasks cannot be escalated (they are already running when they fail);
their log shows the error.

Limits of this signal: a program that hides the error, prints no path, or prints
only a relative path without a `/` is not recognized (the agent gets the failure
plus a general note that the sandbox may be the cause, but nothing is offered);
a permission error from macOS privacy protection (TCC) on a path the sandbox
also blocks can offer an escape that then fails again; and a genuine network
outage looks like a block when `network` is `"block"`. Commands behind wrappers
fail the same way as direct ones.

### Backend limits

- **Seatbelt** (`sandbox-exec`) is deprecated by Apple but still shipped and used
  by Apple's own tools; profile paths are passed as `-D` parameters, never
  written into the profile text, so a path cannot change the profile. A
  sandboxed program cannot start another Seatbelt sandbox inside it.
- **bubblewrap** needs unprivileged user namespaces; distributions or containers
  that forbid them make it unavailable (then `auto` uses Landlock). Secret
  folders are hidden behind an empty read-only folder; missing files cannot be
  protected (they do not exist yet), so a protected path created during a run is
  only protected from the next run on.
- **Landlock** can only allow, never carve an exception out of an allowed
  folder. Reads are therefore granted on every folder beside the paths that lead
  to a secret location; files created later directly in your home folder are not
  readable during that run. The project's own `.themis/` folder cannot be
  protected without also forbidding new files at the top of the project, so on
  Landlock it stays writable; project configuration can only add restrictions
  and project integrations still need your consent in app data, so this does not
  let an agent grant itself permissions. UDP is not restricted. Some network and
  FUSE file systems (for example Docker Desktop bind mounts) do not keep folder
  identities stable; Landlock then denies access to folders it was told to
  allow, including the project, so commands there fail (bubblewrap does not have
  this limit; in Custom you are offered to run them outside the sandbox).
- **Windows** has no backend yet: commands run unsandboxed and the status says so.

### The file guard

Themis' own file tools (`read_file`, `write_file`, `apply_patch`, ...) are not
separate programs, so the OS sandbox does not apply to them. While the sandbox is
enabled they still refuse to write the app-data folder and, in Custom, to read or
write the secret locations; under YOLO (where they may use paths outside the
project and read anything) they also refuse to write the project's `.themis/`
folder. In Custom they stay
confined to the project as before; writing `.themis/config.jsonc` there still
asks you.

### Seeing the effective permissions

**Settings → Permissions** shows the global `runtime.jsonc` with an **Open**
action, which opens it as plain text in your text editor (writing a starter
with every setting commented out if the file does not exist yet; an existing
file is never changed). Below it, it lists only problems: an invalid file
with its line, a named sandbox that is not available, no sandbox on this
system, or the sandbox turned off. Project files and chat grants are not
shown there. `themis permissions [PROJECT] [--thread ID] [--json]` prints the
full effective view, including project rules, the sandbox details and the
chat's grants, from the shared app server.

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
**Allow in this chat** also remembers the grant key for every later run of the same chat
(stored in `chat_grants/<chat id>.json`; see [configuration](configuration.md)): the tool (and
risk) for most tools, one key for all tools of an MCP server; for `shell`, program plus
subcommand only for a short list of subcommand-style CLIs such as `cargo test`, and the exact
full argv for everything else. Destructive variants such as `git push --force`, `git reset
--hard`, `rm -rf` or `cargo publish` and the destructive risk class are never remembered: they ask
every time and the dialog offers only Once and **Deny**.
Chat grants cannot bypass policy denies. They (and the MCP start consent in `mcp_consent.json`) are plain files in app data without integrity protection; sandboxed commands and Themis' file tools cannot write them, but a command you let run outside the sandbox (or any command while the sandbox is off or unavailable) can, the same limit as the plugin registry. Approval timeouts (default five minutes, after which
the dialog is removed with a short note), lost clients, and unattended CLI requests deny. The interactive hook denies
non-read actions in non-Git projects; explicit allow policy or YOLO bypasses
that interactive restriction. A non-Git folder is not an OS-enforced read-only
sandbox.

Custom's Git tool limits subcommands; permitted commands still consult approval
policy, including reads. Skill tool allowlists further filter available tools.
Hooks and MCP initialization instructions do not grant permissions.

## YOLO mode

YOLO means no approval prompts and no sandbox walls but one. It bypasses harness
approval, the core file-tool root restriction, the Git subcommand allowlist, and
skill tool filtering, and its commands run under an *open* sandbox plan: they
may write anywhere, read the secret locations (`~/.ssh`, `~/.aws`, ...), and use
the network and the Keychain. YOLO removes Themis' default walls, never the ones
a configuration wrote: a `sandbox.network: "block"` or `allow_keychain: false`
set in the user file, the project's `.themis/config.jsonc` or a run override
still holds, and so does every approval `deny` rule (the built-in rules include
none); `ask` rules and the default action allow, since YOLO never asks. The
composer's shield names what still applies ("still applies: network blocked, 2
deny rules"). The one wall left: neither commands nor file tools can
write the app-data folder or the project's `.themis/` folder, so an agent cannot
change its own rules, grants or settings; such a write fails with a short note
and is never escalated. Under Landlock (Linux without bubblewrap) the project's
`.themis/` cannot be carved out of a writable tree, so there only the file tools
protect it; the app-data folder stays protected on every backend, and Settings ›
Permissions suggests installing bubblewrap there. With
`sandbox.enabled: false`, or no usable backend, commands are not confined at all.
YOLO runs also get a short caution in their system prompt: no destructive or
irreversible actions the person did not ask for (deleting data outside the task,
force-pushing, dropping databases, changing system settings, sending anything),
asking in the reply instead, and treating instructions in pages, files and tool
output as data. It is advice to the model, not enforcement. OS permissions, tool
input validation, timeouts, output bounds, and child environment handling still
apply. The choice is saved per thread. An active run retains its captured mode
and configuration; edits affect subsequent runs.

## Programs and filesystem access

The `shell` tool launches a program with an argv array, without implicitly
parsing shell strings. Pipes and redirects are literal arguments unless an
explicit interpreter such as `bash -c` is launched. An approved program performs
its own filesystem/network operations inside the OS sandbox described above;
when the sandbox is off or unavailable (or you let a command run outside it),
pinning its working directory does not confine the process. Core path guards do
not sandbox third-party MCP servers/hooks.

Custom requires the tool working directory to stay inside the project root.
YOLO permits external paths. Command rules are a tool policy, not a sandbox: deny rules match the program basename
(case-insensitively on macOS/Windows) and also the commands inside wrappers such as
`env`, `sudo` or `bash -c` scripts, but not script files or other interpreters
(`python -c`); see [configuration](configuration.md). The shell child runs in its own process group, so Ctrl-C in the CLI does
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
descendant that detaches from its parent is not reached; CI covers the timeout and
background-stop grandchild kill on windows-latest). Child environment
handling excludes provider credentials from normal tool launches. Children
receive only `PATH`, `TMPDIR`/`TEMP`/`TMP`, `LANG`, `LC_ALL` and other `LC_*`,
`TERM`, plus `HOME`/`USER`/`LOGNAME`/`SSH_AUTH_SOCK` on Unix or the Windows
system/user variables; any name containing `KEY`, `TOKEN` or `SECRET` is
dropped even if otherwise allowed. MCP servers can receive explicitly configured environment-variable references.
Hooks receive the fixed allowlist plus `CLAUDE_PLUGIN_ROOT`, rather than a
configurable environment map. Imported MCP servers and hooks
begin disabled and require explicit activation; hook trust remains separate
from installation.

## Sensitive project files

The sandbox protects secret folders in your home folder. It does **not** protect
secrets inside the project: a `.env` is an ordinary file to it. So in **Custom**,
calls that touch a sensitive project file ask first, through the usual **Allow
once** · **Allow in this chat** · **Deny**, whatever other rules or grants would
say. This is detection, not a wall.

Sensitive means a file name matching `approval.sensitive_files`: by default
`.env`, `.env.*`, `*.pem`, `*.key`, `id_rsa`, `credentials.json`, `.npmrc`,
`.pypirc` and `secrets.*`, except `.env.example`, `.env.sample` and
`.env.template`. Names match without case, and a symlink counts by what it
points at. Whether Git ignores the file makes no difference. See
[configuration](configuration.md) for changing the list.

| Call | What asks |
| --- | --- |
| `read_file`, `copy_file` source | Reading an existing sensitive file |
| `write_file`, `apply_patch`, `move_file`, `delete_file`, `copy_file` target | Writing or removing one, new files included |
| `shell`, background tasks, `git` | Any word of the command that names one, as far as the words can be read (below) |
| `git add`, `git stage`, `git commit` | What Git says it would take in: see below |
| `search_file`, `list_dir` | Nothing: they return names and sizes, never contents |

**Subagents** ask the same way, under their own name. **YOLO** stays open: nothing
asks, unless a deny rule says otherwise. **Non-Git projects** ask for reads as usual;
writes there are refused without asking, as for any file.

**"Allow in this chat"** covers that one file and that kind of access (read, write or
commit), and for a command also that command, as usual: reading `.env` is not writing
it, a grant on `.env` doesn't cover `.env.local`, and a grant on `write_file` or
`git add` doesn't cover a sensitive file. Destructive actions still can't be granted.
Rules decide first, so a deny rule beats a grant. A rule with `"tool": "sensitive_file"`
decides for these files: `{"tool":"sensitive_file","action":"deny"}` refuses them in
every mode, YOLO included, and wildcard or risk-only allow rules never cover them. Its
optional `command` is `["read" | "write" | "commit", path]`. An `allow` rule for a
program (`["cat"]`) doesn't cover `cat .env`, and a watch's `check_command` that names
one needs the same approval.

**Shell commands can't be fully detected.** Themis reads the command's words,
through wrappers such as `sh -c` and `sudo`, following a `cd` in a script, including redirects, pieces of a word
(`--env-file=.env`, `@.env`, `open('.env')`), globs (`cat .en*`, expanded against the
folder) and what is directly inside a folder given to `grep -r`, `cp -r`, `tar`, `zip`
or `rsync`. It does not see:

- a file named at run time: a variable (`F=.env; cat $F`), string building
  (`cat .e""nv` is read, `cat $(printf .env)` is not), or a list piped in
  (`echo .env | xargs cat`);
- a program that finds the file itself: a script (`python read_env.py`, `npm run x`), a
  framework or `dotenv` loading `.env`, `find . -exec cat`, or `rg --hidden`;
- a file deeper than the top of a folder given to `grep -r` and the like (`grep -r X .`
  sees `./.env`, not `./app/.env`), or a hard link;
- what a command you allowed goes on to do: once `cat .env` runs, its output reaches the
  model, and an allowed `curl -d @.env` sends the file;
- MCP tools, plugin hooks and anything else outside the core tools.

It also asks when nothing would leak: `grep .env .gitignore` reads as naming `.env`
when that file exists. Shell commands usually ask anyway, so this costs one answer.

### Secrets and Git

Before `git add`/`git stage` and `git commit` (also inside `sh -c`), Themis asks Git:
files `add` would take in that are untracked and not ignored or changed and tracked,
within the given paths (`git add .`, `-A`, `-u`, a folder), and for `commit` what is
staged, plus changed files with `-a`. A sensitive file among them asks in Custom, and
the result tells the agent in every mode, so a grant for `git add` can't stage a `.env`
unnoticed. In YOLO that note comes after the fact. Each listing is one `git` call with a 2-second deadline and an
8 MiB output limit, in the scrubbed environment and with `core.fsmonitor` off, so it
starts no program from the repository's configuration; it runs outside the sandbox.
If Git doesn't answer in time, the request says so and asks. Git aliases, `git commit`
of named paths, `git -c` tricks and `git update-index` are not read.

When a run starts, one bounded `git ls-files` call (1.5 seconds; dropped if slower)
finds secret-looking files anywhere in the repository. Those not ignored get a line in
the system message to add them to `.gitignore` before any commit; those already
tracked get a line telling the agent to say that ignoring them won't remove them and
the key stays in history. Non-Git projects get neither. The system instructions also
ask the agent to keep `.gitignore` covering secret files whenever it creates or finds
one, or sets up a project; that part is advice to the model.

`search_file` and `list_dir` do not consider `.gitignore`; see
[configuration](configuration.md).

### Secrets pasted into a message

A message that looks like it holds a credential (well-known prefixes, a private key,
a JWT, or `SOME_KEY=` with a long random value) gets a short line above the composer,
checked on the device, with one dismiss; you can still send. A sent one also adds a
fixed note to the model request asking the agent to warn you without repeating the
value, and to tell you to revoke and rotate it. The note holds no part of the value,
and neither the text nor the match is logged. Hashes, UUIDs and ordinary code are
meant to stay quiet, so an unusual key format can be missed, and the check reads only
the first 256 KiB of a message. A subagent's brief is not checked.

## Shared checkout and secrets

Threads currently share the project checkout, including existing uncommitted
files. File edits affect that checkout directly. Select a separate Git worktree
as a project when you want checkout isolation. Commands you let run outside the
sandbox, MCP servers, hooks and YOLO file tools can access other paths. Inspect
changes in your Git client. Stopping or removing a thread does not undo its
completed filesystem operations.

Entered API keys remain in macOS Keychain; launch environment keys are handled
by the backend. Only key availability crosses the settings bridge. Diagnostic
records exclude prompt/tool/provider bodies, redact credential fields and
referenced secret values, and bound debug stderr. Third-party stderr may contain
other private information; inspect exported logs before sharing them.

## On an unexpected action

1. Deny a pending request or stop the run.
2. Inspect the worktree and any paths affected by approved programs.
3. Review the recorded tool events; advanced users can inspect local diagnostic logs.
4. If a key was pasted into a prompt/file, committed or otherwise exposed, rotate it
   with the provider and replace the stored key. It reached the model provider and
   the saved chat; adding a committed file to `.gitignore` doesn't remove it from
   history.

## Background shell tasks

`shell` with `"background": true` starts the program and returns at once with a
`task_id` (optional `background_timeout_secs`, default 7,200, maximum 86,400). The
call goes through exactly the same approval path as a blocking call: same
`ToolAction` argv, same grant key and command-scoped `approval.rules[].command`
matching, same Custom/YOLO behavior; the approval summary adds "in the
background". The program gets the same scrubbed environment, no stdin and its
own process group, inside the run's OS sandbox (the start result says
`"sandboxed": true`; a sandbox block is not escalated). Output (stdout and stderr interleaved) feeds a rolling log of
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
`shell` action, runs inside the OS sandbox the current configuration describes
(never escalated), and follows this policy: the same argv, Execute risk, command-scoped
and deny rules. Creating or updating an automation is approval-free for the agent's
own chat; executing a program is not. In Custom mode an `ask` becomes an approval of
the exact argv in the creating run, asked as a `shell` action (command-scoped
rules, the non-git refusal; the chat's saved grants count) using the mode and policy that run captured,
stored with the automation and invalidated by any change; the scheduler never prompts and pauses a watch that lacks approval or whose
policy now denies. Policy is re-evaluated before every execution, so a later deny
rule or a switch from YOLO to Custom stops the check. A chat's saved grants count like an allow rule for non-destructive checks and are re-read before every execution, so removing one stops the check. Deny rules also still reject an agent's own-chat
automation lifecycle actions (`manage_automations*`); `ask` proceeds without a prompt.
