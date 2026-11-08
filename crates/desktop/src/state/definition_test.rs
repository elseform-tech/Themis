//! Testing a hook or an MCP server definition: start its command once and report what came back.
//!
//! The app server runs this for the person (`themis mcp|hook test`, the `manage_integrations`
//! tool after its approval). In an agent's shell the `themis` command runs it itself, inside the
//! run's sandbox, because the server refuses an agent's request to start a command outside it.

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};

/// Runs `test_mcp` (`args["server"]`) or `test_hook` (`args["hook"]`) in `root`. A test an agent
/// asks for runs a definition the agent may have written, so it never reads secrets.
pub(crate) async fn run(
    action: &str,
    args: &Value,
    root: &Path,
    from_agent: bool,
) -> anyhow::Result<Value> {
    match action {
        "test_mcp" => {
            let spec: themis_core::plugins::McpServer =
                serde_json::from_value(args["server"].clone())?;
            if from_agent && !spec.key_names().is_empty() {
                anyhow::bail!(
                    "A test can't read secrets. Save the connection and have the person enable it."
                );
            }
            let mut connection =
                themis_core::plugins::connections::Connection::connect(&spec, root).await?;
            Ok(json!({"connected": true, "tools": connection.tools().await?}))
        }
        "test_hook" => {
            let hook: themis_core::plugins::Hook = serde_json::from_value(args["hook"].clone())?;
            // This explicitly requested test runs its displayed command; ordinary runs use approvals.
            let approvals: Arc<dyn themis_core::tools::ApprovalHook> =
                Arc::new(themis_core::tools::AllowAllHook);
            themis_core::plugins::hooks::run(
                &hook,
                root,
                &json!({"event": hook.event, "test": true}),
                &approvals,
            )
            .await
            .map_err(|error| {
                // A test shows what the hook printed: that is how its author finds the problem.
                let printed = error
                    .downcast_ref::<themis_core::plugins::hooks::Failed>()
                    .map(|failed| failed.stderr().to_owned())
                    .filter(|printed| !printed.is_empty());
                match printed {
                    Some(printed) => error.context(format!("It printed: {printed}")),
                    None => error,
                }
            })
        }
        _ => anyhow::bail!("Unknown test {action}"),
    }
}
