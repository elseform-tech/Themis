---
name: mcp
description: Install, import, configure, create, test and troubleshoot local or remote MCP servers, including pasted JSON configurations.
---

# MCP

Use global scope for new installations and personal packages so they are available in every chat. Preserve the scope of existing items when updating them.

Create, configure and troubleshoot integrations in chat through the shared management tools. Integrations provides browsing, one-click plugin/skill installation, and basic lifecycle controls. Use shared tools for requested changes. Runtime and MCP configuration use advanced files or CLI APIs; raw configuration and logs stay out of the desktop UI.

Inspect existing servers with `list_plugins`. Prefer an existing maintained server over building one. Accept a user's configuration file, JSON, command, endpoint, package or repository; do not assume every source is a repository URL.

Use `manage_integrations` import_json for supported mcpServers/OpenCode mcp configuration and import_path for files. JSON describes a connection or launch command, not proof that server software exists. Check required runtimes and pinned package versions before executing. Import never grants executable approval or account access. Configure required environment variables without embedding their values in bundles or messages.

Use test_mcp after configuring an enabled server; inspect discovered tools and exercise a harmless tool when appropriate. Diagnose missing runtimes, process errors, protocol errors, timeouts and authentication separately. Use `set_component_enabled` with the package name, kind `mcp`, the connection key as id, and enabled boolean for availability. A server that runs a command is switched on by the user alone: Themis saves it switched off and refuses switching it on from you in every mode, so ask the user to switch it on in Integrations, then test it. Do not require a skill selection to use MCP tools. For unsupported OAuth flows, report the limitation instead of inventing a connected account.

To check every installed integration after a change, run `themis doctor --integrations` in the shell: it lists each problem with where it is and a one-line fix (`--no-start` checks without starting MCP servers).
