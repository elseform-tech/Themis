---
name: skills
description: Find, import, install, create, inspect, enable, disable, update and troubleshoot reusable SKILL.md instructions.
---

# Skills

A skill is Markdown instructions, with optional helper scripts, that a run loads when its description fits. Do the work as an agent; do not ask the person to fill in a form.

Two kinds, and the person's words decide which:
- **The person's own skill** (the default: every chat, only them, only Themis): saved in a plugin in `{{global_plugins}}` with `themis skill …` in your shell. Never edit the plugin store's files by hand (Themis refuses it).
- **A skill for this project, shared with the team** (when they say so: "for this project", "for the team", "in the repo"): a `SKILL.md` folder in the repository, `{{project_skill_dir}}/NAME/SKILL.md`, written with your file tools and committed like any project file. Other coding agents read it too.

Skills Themis also finds on disk, read-only through `themis`: personal folders {{personal_skill_folders}}, and this project's {{project_skill_folders}}.

## Find

- `themis skill list` lists every skill a run can use, with its plugin and scope. Look before creating a duplicate.
- `themis skill list PLUGIN` prints that plugin's skill definitions in full (`id`, `name`, `description`, `instructions`, `allowedTools`, `scripts`). A skill found on disk is its `SKILL.md`: read it with `read_file`.
- `themis skill --help` shows every field of a definition with an example.

## Change

1. Write the definition as one JSON object to a scratch file, for example `"$TMPDIR/skill.json"`: `id`, `name`, `instructions`, and optionally `description`, `allowedTools` (empty restricts nothing and never grants a permission) and `scripts` (`name`, `content`; plain file names). Give it a clear trigger in the description, then inputs, steps, expected output and how to check it. Add scripts only when a fixed program helps. No secrets, no permission bypasses, no speculative scaffolding.
2. Check it: `themis skill verify DEF` (DEF: the file, `-` for stdin, or the JSON inline in quotes) reports every problem at once with the exact shape. Fix them all. Test any script with made-up inputs.
3. Save it: `themis skill save PLUGIN DEF`. Use `personal` as PLUGIN unless the person names another plugin they own; a missing plugin is created. Saving a skill with an existing `id` replaces it and keeps the plugin's other skills. Start an update from `themis skill list PLUGIN` and keep its `id`.
4. Remove one only when asked: `themis skill delete PLUGIN ID`. A skill found on disk is not in the store: tell the person which file it comes from.

A project skill instead: write `{{project_skill_dir}}/NAME/SKILL.md` (NAME in lowercase letters, digits and `-`) as YAML front matter with `name` and `description`, then the instructions; put helper scripts beside it. To change or remove one, edit or delete that folder. It shows as a Project skill on the next refresh.

Import a skill folder or bundle with `themis plugin inspect SOURCE`, then `themis plugin import SOURCE` (see the plugins skill). A marketplace plugin's skills are not edited in place.

## Verify

- `themis skill list PLUGIN` again, and `themis skill list` to see it among the skills a run gets: report what the store says.
- `themis doctor --integrations --no-start` after every change: it lists every problem with where it is and a one-line fix.
- Report the saved skill, whether it is the person's own or the project's, the check's result and what you did not test. A skill appears in the `/` menu on the next refresh and reaches the next run. A skill never bypasses Themis approvals.