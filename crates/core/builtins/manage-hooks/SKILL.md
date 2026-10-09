---
name: manage-hooks
description: Create, import, install, configure and troubleshoot event hooks with bounded execution and explicit permissions.
---

# Manage hooks

Use global scope for new installations and personal packages so they are available in every chat. Preserve the scope of existing items when updating them.

Create, configure and troubleshoot integrations in chat through the shared management tools. Integrations provides browsing, one-click plugin/skill installation, and basic lifecycle controls. Use shared tools for requested changes. Runtime and MCP configuration use advanced files or CLI APIs; raw configuration and logs stay out of the desktop UI.

Inspect installed hook definitions using `list_plugins`. The supported runtime events are RunStart, BeforeTool, AfterTool, BeforeCompaction and RunEnd. Users configure handlers for these events; they do not create new runtime events.

Create a personal bundle through `manage_integrations` save, with hook name, event, command, enabled, timeout_seconds and failure behavior. Keep executable hooks disabled until the user requests enablement; execution follows the active YOLO/Custom policy; enablement and hook trust remain separate. Use a matcher to restrict tool events when appropriate. Preserve existing configuration and use expectedRevision to avoid overwriting concurrent edits.

Use `set_component_enabled` with the package name, kind `hook`, the hook name as id, and enabled boolean. Use `test_hook` with the hook configuration for an explicitly requested execution test. Review imported event mappings and report unsupported semantics. Test using synthetic event input and safe resources. Before events can block; after events cannot undo completed operations. Bound execution time and output, avoid recursive invocation, and define what happens on failure. Review changed commands before enabling them; execution follows the active YOLO/Custom policy. Never approve a hook on the user's behalf.
