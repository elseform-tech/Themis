---
name: init
description: Write, create, initialise or improve a project's AGENTS.md or CLAUDE.md, the instructions coding agents read before working in it; also /init.
---

# Init

Write or improve AGENTS.md at the project root.

Explore first, with bounded reads: the top-level folders, the README, build and package manifests, CI workflows, linter and formatter settings, and a few source and test files. Skip generated and vendored folders.

If AGENTS.md exists, improve it and keep the person's own rules. If only CLAUDE.md exists, build AGENTS.md from it rather than duplicating it.

Use these sections, in this order:

```markdown
# AGENTS.md

## Product
Two or three sentences on what this project is.

## Project structure
The main folders and what belongs in each, then the 5 to 10 files to open first, one line each.

## Build, test and lint
Commands that actually exist here, as the manifests and CI show them.

## Coding conventions
What the code does: formatting, naming, test layout.

## Security
Never commit secrets. Keep `.env` in `.gitignore`. Where secrets belong in this project.

## How to work
Write tests first, get changes reviewed, and verify before claiming something works.
```

Aim for about 1,500 tokens; never go past about 3,000 tokens (12 KB). When you are done, say what you wrote or changed.
