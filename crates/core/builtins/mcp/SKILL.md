---
name: mcp
description: Install, import, configure, create, test and troubleshoot local or remote MCP servers, including pasted JSON configurations.
---

# MCP

MCP servers belong to plugins and are managed with `themis mcp …` in your shell. Integrations in the app browses them and switches them on and off; changes in chat go through `themis`.

Where they live: in plugins in `{{global_plugins}}`, the person's own, in every chat and project. A project holds none: never write a `--scope` or a project file for them. Asked for one "only for this project", add it as the person's own and tell them plainly that it applies in every project. A "local" server means one that runs a command on this machine (as opposed to a remote `url`), not one for this project. Read them through `themis`; never edit those files by hand (Themis refuses it).

## Find

- `themis mcp list` lists every server with its plugin; `themis plugin list` shows the plugins and whether each server is on.
- `themis mcp list PLUGIN` prints that plugin's server definitions in full: `command`, `args`, `cwd`, `url`, environment names and `enabled`. Read this before changing or diagnosing a server; do not search the disk for its configuration.
- `themis mcp --help` shows every field of a definition with an example.

Prefer an existing maintained server over building one. Accept what the person gives: a configuration file, pasted JSON, a command, an address, a package or a repository.

## Change

- **One server:** give its definition as one JSON object, inline in quotes, from stdin with `-`, or as a file: `themis mcp add PLUGIN NAME '{"command": "node", "args": ["server.js"]}'` (or `'{"url": "https://…"}'`) for a new one, `themis mcp update PLUGIN NAME '<json>'` to replace one. Use `personal` as PLUGIN for the person's own; a missing plugin is created. In Integrations the person sees these as Yours, not inside a plugin, so never send them to a `personal` plugin. To fix one, start from `themis mcp list PLUGIN`, change only what is wrong and keep the rest.
- **A standard `.mcp.json`** (`{"mcpServers": {…}}`, a file or pasted JSON): `themis mcp add PLUGIN FILE` (or `'<json>'`) adds every server in it, converted the way an import converts them, and says which it added and which it could not use (`--allow-partial` adds the rest). With a NAME, `themis mcp add PLUGIN NAME FILE` adds only that server. Never write a `.mcp.json` into the project for Themis: it does not read one there.
- **Off or gone:** `themis mcp disable PLUGIN NAME`; `themis mcp delete PLUGIN NAME` only when asked.

Each definition is checked first and every problem is reported at once with the exact shape; fix them all and run it again. JSON describes a launch command or an address, not proof that the software exists: check the runtime it needs (`node`, `uvx`, `python3`) and pinned package versions. Keys are never written into a definition: `env` maps a variable to the name of a secret, and the person stores the secret in Settings.

A server that runs a command is saved switched off and only the person switches it on: Themis refuses it from you in every mode. Tell the person which server to switch on in Integrations, then test it. A server by address may be switched on with `themis mcp enable PLUGIN NAME`. For sign-in flows Themis does not support, say so instead of inventing a connected account.

## Verify

- `themis mcp list PLUGIN` again: report what the store says, not what you meant to save.
- `themis mcp test PLUGIN NAME` starts the server inside your sandbox and lists its tools. Diagnose a missing runtime, a process error, a protocol error, a timeout and missing authentication separately. A failure may be the sandbox (network, files outside the project): then ask the person to run the same test in their own terminal.
- `themis doctor --integrations` after every change: it starts each switched-on server and lists every problem with where it is and a one-line fix (`--no-start` checks without starting servers).

Tools from a server reach the next run, not the current one. Do not require a skill selection to use MCP tools.