---
name: manage-skills
description: Find, import, install, create, inspect, enable, disable, update and troubleshoot reusable SKILL.md instructions.
---

# Manage skills

Inspect `list_plugins` and the available skill catalog before creating duplicates. Read relevant SKILL.md instructions and supporting Markdown through `read_file`. Skills in user scope are available across chats; project skills apply to chats in that project.

Import local skill files/folders or supported marketplace/repository bundles through `manage_integrations`. Create with `save_skill`: scope, plugin, expectedRevision, and a skill object containing id, name, description, instructions, allowedTools, and scripts. Keep the skill concise, with a clear trigger and verification steps. Use scripts only for concrete deterministic work. Preserve IDs on update and validate through the backend.

Use `set_component_enabled` to enable or disable an individual skill. Use remove_component to remove the requested bundled skill while preserving its siblings. Discovered source files remain read-only; disable those skills or explain how to remove the original file. Read supporting files only as needed. Never claim an imported skill works without testing the relevant behavior. Secrets and permission bypasses do not belong in instructions.
