---
name: manage-hooks
description: Create, import, install, configure and troubleshoot event hooks with bounded execution and explicit permissions.
---

# Manage hooks

Inspect installed hook definitions using `list_plugins`. The supported runtime events are RunStart, BeforeTool, AfterTool, BeforeCompaction and RunEnd. Users configure handlers for these events; they do not create new runtime events.

Create a personal bundle through `manage_integrations` save, with hook name, event, command, enabled, timeout_seconds and failure behavior. Keep executable hooks disabled until the user requests enablement; runtime approval remains required. Use a matcher to restrict tool events when appropriate. Preserve existing configuration and use expectedRevision to avoid overwriting concurrent edits.

Review imported event mappings and report unsupported semantics. Test using synthetic event input and safe resources. Before events can block; after events cannot undo completed operations. Bound execution time and output, avoid recursive invocation, and define what happens on failure. A changed command requires fresh execution approval. Never approve a hook on the user's behalf.
