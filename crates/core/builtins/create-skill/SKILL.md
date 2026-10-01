---
name: create-skill
description: Design, create, validate, and update reusable skills in a local or global personal plugin.
---

# Create skill

Turn the user's task into a reusable skill. Perform the work as an agent; do not ask the user to fill in a manual authoring form.

1. Read existing plugins with `list_plugins`. Reuse a user-owned plugin when appropriate. Never edit a marketplace package in place.
2. Use local scope for the current project unless the user requests global scope. Global skills are available in every project. The default destination plugin is `personal`.
3. Design concise instructions with a clear trigger, inputs, workflow, expected outputs, and verification. Include scripts only when deterministic execution is useful. Do not embed secrets, credentials, or speculative scaffolding.
4. Write a JSON skill draft in the project. It contains `id`, `name`, `description`, `instructions`, `allowedTools`, and `scripts`. Each script has `name` and `content`. IDs and script names are single safe file names; scripts cannot use parent directories or absolute paths. Empty `allowedTools` adds no restriction and never grants permissions.
5. Run `python3 .themis/skills/create-skill/verify.py <draft.json>`. Fix every reported error. Test generated scripts with synthetic inputs through approved tools. The verifier checks structure and safety, not task quality; report unverified behavior explicitly.
6. Persist with `save_skill`, passing scope, plugin, skill, and the plugin's current `expectedRevision` from `list_plugins`. Use null for a new plugin. If an edit conflicts, reload and reconcile instead of overwriting.
7. Report the saved skill, local/global scope, validation result, script checks, and any limitations. It becomes available in slash autocomplete on the next refresh.

When updating a skill, preserve its ID and other skills in the plugin. Delete only when explicitly requested by the user. A generated skill must never bypass Themis approvals.
