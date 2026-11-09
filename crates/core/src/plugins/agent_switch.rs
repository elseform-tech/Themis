//! What an agent may not do to integrations, in any mode (YOLO included): leave a hook or a
//! command-based MCP server switched on. Both run code outside the sandbox, so switching them on
//! is the person's. A store made with [`PluginStore::for_agent`] saves, imports and installs new
//! or changed ones switched off and refuses to switch one on, inside the same registry write, so
//! no other call can slip in between. A connection by URL keeps the consent rule: an agent may
//! leave it on, and it still waits for the person's approval before it starts.

use super::*;

/// The error an agent gets when it tries to switch one on.
fn refused() -> anyhow::Error {
    anyhow::anyhow!(crate::prompts::AGENT_SWITCH_ON_REFUSED)
}

/// An MCP server that starts a local program, rather than connecting to a URL.
fn runs_a_command(server: &McpServer) -> bool {
    server.command.is_some()
}

impl PluginStore {
    /// For an agent: every hook and command-based MCP server of `spec` that is new or changed
    /// since `previous` (or was off there) is switched off; an unchanged one keeps its switch.
    pub(super) fn keep_new_parts_off(&self, previous: Option<&Plugin>, spec: &mut PluginSpec) {
        if !self.agent_saves {
            return;
        }
        let previous = previous.map(|plugin| &plugin.spec);
        for (name, server) in spec.mcp.iter_mut().filter(|(_, s)| runs_a_command(s)) {
            let kept = previous
                .and_then(|previous| previous.mcp.get(name))
                .is_some_and(|old| old.enabled && marketplace::same_definition(old, &*server));
            server.enabled &= kept;
        }
        for hook in &mut spec.hooks {
            let kept = previous
                .and_then(|previous| previous.hooks.iter().find(|old| old.name == hook.name))
                .is_some_and(|old| old.enabled && marketplace::same_definition(old, &*hook));
            hook.enabled &= kept;
        }
    }

    /// For an agent: refuses switching on a hook or a command-based MCP server of `plugin` that
    /// is off. Switching off, skills, connections by URL and what is already on pass.
    pub(super) fn refuse_switching_on(
        &self,
        plugin: &Plugin,
        kind: &str,
        id: &str,
        enabled: bool,
    ) -> anyhow::Result<()> {
        if self.agent_saves && switches_on_part(plugin, kind, id, enabled) {
            return Err(refused());
        }
        Ok(())
    }

    /// For an agent: refuses switching on a plugin that is off while it holds a hook or a
    /// command-based MCP server that is on, which would start running with it.
    pub(super) fn refuse_enabling_plugin(
        &self,
        plugin: &Plugin,
        enabled: bool,
    ) -> anyhow::Result<()> {
        if self.agent_saves && enabling_starts_code(plugin, enabled) {
            return Err(refused());
        }
        Ok(())
    }
}

/// Switching on a hook or a command-based MCP server of `plugin` that is off.
fn switches_on_part(plugin: &Plugin, kind: &str, id: &str, enabled: bool) -> bool {
    enabled
        && match kind {
            "hook" => plugin.spec.hooks.iter().any(|h| h.name == id && !h.enabled),
            "mcp" | "connection" => plugin
                .spec
                .mcp
                .get(id)
                .is_some_and(|server| runs_a_command(server) && !server.enabled),
            _ => false,
        }
}

/// Switching on a plugin that is off while it holds a hook or a command-based MCP server that
/// is on, which would start running with it.
fn enabling_starts_code(plugin: &Plugin, enabled: bool) -> bool {
    let starts_code = plugin.spec.hooks.iter().any(|hook| hook.enabled)
        || plugin
            .spec
            .mcp
            .values()
            .any(|server| server.enabled && runs_a_command(server));
    enabled && !plugin.enabled && starts_code
}
