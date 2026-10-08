---
name: manage-skills
description: Find, import, install, create, inspect, enable, disable, update and troubleshoot reusable SKILL.md instructions.
---

# Manage skills

Use global scope for new installations and personal packages so they are available in every chat. Preserve the scope of existing items when updating them.

Create, configure and troubleshoot integrations in chat through the shared management tools. Integrations provides browsing, one-click plugin/skill installation, and basic lifecycle controls. Use shared tools for requested changes. Runtime and MCP configuration use advanced files or CLI APIs; raw configuration and logs stay out of the desktop UI.

Inspect `list_plugins` and the available skill catalog before creating duplicates. Read relevant SKILL.md instructions and supporting Markdown through `read_file`. Skills in user scope are available across chats; project skills apply to chats in that project.

Import local skill files/folders or supported marketplace/repository bundles through `manage_integrations`. Create with `save_skill`: scope, plugin, expectedRevision, and a skill object containing id, name, description, instructions, allowedTools, and scripts. Keep the skill concise, with a clear trigger and verification steps. Use scripts only for concrete deterministic work. Preserve IDs on update and validate through the backend.

Use `manage_integrations` with `set_component_enabled`, the package name, kind `skill`, the skill's stored id, and enabled boolean. Use `remove_component` with the same name/kind/id to remove the requested skill while preserving its siblings. Discovered source files remain read-only; disable those skills or explain how to remove the original file. Read supporting files only as needed. Never claim an imported skill works without testing the relevant behavior. Secrets and permission bypasses do not belong in instructions.
