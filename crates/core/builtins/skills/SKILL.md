---
name: skills
description: Find, import, install, create, inspect, enable, disable, update and troubleshoot reusable SKILL.md instructions.
---

# Skills

Use global scope for new installations and personal packages so they are available in every chat. Preserve the scope of existing items when updating them.

Create, configure and troubleshoot integrations in chat through the shared management tools. Integrations provides browsing, one-click plugin/skill installation, and basic lifecycle controls. Use shared tools for requested changes. Runtime and MCP configuration use advanced files or CLI APIs; raw configuration and logs stay out of the desktop UI.

Inspect `list_plugins` and the available skill catalog before creating duplicates. Read relevant SKILL.md instructions and supporting Markdown through `read_file`. Skills in user scope are available across chats; project skills apply to chats in that project.

Import local skill files/folders or supported marketplace/repository bundles through `manage_integrations`.

To create or update a skill, do the work as an agent; do not ask the user to fill in a manual authoring form.

1. Read existing plugins with `list_plugins`. Reuse a user-owned plugin when appropriate; the default is `personal`. Never edit a marketplace package in place.
2. Design concise instructions with a clear trigger, inputs, workflow, expected outputs, and verification. Include scripts only when deterministic execution is useful. Do not embed secrets, credentials, or speculative scaffolding.
3. Write a JSON skill draft in the project. It contains `id`, `name`, `description`, `instructions`, `allowedTools`, and `scripts`. Each script has `name` and `content`. IDs and script names are single safe file names; scripts cannot use parent directories or absolute paths. Empty `allowedTools` adds no restriction and never grants permissions.
4. Run `python3 .themis/skills/skills/verify.py <draft.json>`. Fix every reported error. Test generated scripts with synthetic inputs through approved tools. The verifier checks structure and safety, not task quality; report unverified behavior explicitly.
5. Persist with `save_skill`, passing scope, plugin, skill, and the plugin's current `expectedRevision` from `list_plugins`. Use null for a new plugin. If an edit conflicts, reload and reconcile instead of overwriting.
6. Report the saved skill, its scope, the validation result, script checks, and any limitations. It becomes available in slash autocomplete on the next refresh.

When updating a skill, preserve its ID and other skills in the plugin. Use local scope only when explicitly requested. A generated skill must never bypass Themis approvals.

Use `manage_integrations` with `set_component_enabled`, the package name, kind `skill`, the skill's stored id, and enabled boolean. Use `remove_component` with the same name/kind/id to remove the requested skill while preserving its siblings. Discovered source files remain read-only; disable those skills or explain how to remove the original file. Read supporting files only as needed. Never claim an imported skill works without testing the relevant behavior. Secrets and permission bypasses do not belong in instructions.
