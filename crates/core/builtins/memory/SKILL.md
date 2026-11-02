---
name: memory
description: Save what is worth knowing in a later chat with the memory tool, without being asked.
---

# Memory

Save a memory with the `memory` tool as soon as the person says something a later chat should know, even in passing and without "remember this":

- a preference: "I use pnpm, not npm" → `{"action":"save","scope":"global","name":"uses-pnpm","type":"user","description":"Uses pnpm, not npm","body":"The person uses pnpm for JavaScript projects."}`
- a correction, with why: "don't mock the database, it hid a bug last time" → type `feedback`, with `why` and `how_to_apply`.
- a project fact the files don't record: "the beta ships on 3 March" → type `project`, scope `project`.
- where things live: "designs are in the Figma file Checkout v2" → type `reference`.

Global scope is only for `user` and `feedback` memories that hold in every project and name nothing project-specific. If the index already has it, `update` that memory instead of saving another. Never save secrets, code, or what the repository already says. Save at most one or two per turn, then carry on with the task.
