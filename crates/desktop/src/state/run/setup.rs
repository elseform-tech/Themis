//! Run setup helpers: the run's workspace head, the project secrets note, the hooks' permission
//! mode and MCP tool names.

use super::*;

impl AppState {
    /// Warns the agent about secret-looking files the repository could take in or already
    /// holds. One bounded `git` call per run; its cost and counts go to the diagnostics.
    pub(super) async fn project_secrets_note(
        &self,
        config: &themis_core::configuration::RuntimeConfig,
        root: &std::path::Path,
        thread_id: &str,
        run_id: &str,
    ) -> String {
        use themis_core::tools::sensitive::name_list;
        let matcher = config.approval.sensitive_matcher(Some(root));
        let started = std::time::Instant::now();
        let scan = tokio::task::spawn_blocking(move || matcher.scan_project())
            .await
            .ok()
            .flatten();
        let _ = self.inner.diagnostics.append_with_level(
            "approval",
            "secret_scan",
            "info",
            json!({"thread_id": thread_id, "run_id": run_id,
                "elapsed_ms": started.elapsed().as_millis(), "checked": scan.is_some(),
                "unignored": scan.as_ref().map_or(0, |scan| scan.unignored.len()),
                "tracked": scan.as_ref().map_or(0, |scan| scan.tracked.len())}),
            Some(&config.logging.level),
        );
        let Some(scan) = scan else {
            return String::new();
        };
        let mut note = String::new();
        for (files, prompt) in [
            (
                &scan.unignored,
                themis_core::prompts::DESKTOP_SECRETS_UNIGNORED,
            ),
            (&scan.tracked, themis_core::prompts::DESKTOP_SECRETS_TRACKED),
        ] {
            if !files.is_empty() {
                note.push_str(&themis_core::prompts::fill(
                    prompt,
                    &[("files", &name_list(files))],
                ));
                note.push('\n');
            }
        }
        note
    }
}

/// The head of every desktop run's additions to the system message: its workspace, then where
/// integrations are managed (a tool advertises itself in every request; the `themis` CLI does not).
pub(super) fn run_head(workspace: &str) -> String {
    format!(
        "{}\n{}",
        themis_core::prompts::fill(
            themis_core::prompts::DESKTOP_RUN_WORKSPACE,
            &[("workspace", workspace)]
        ),
        themis_core::prompts::DESKTOP_RUN_INTEGRATIONS
    )
}

/// The mode in the words Claude's hooks read.
pub(in crate::state) fn permission_mode(
    mode: themis_core::configuration::ApprovalMode,
) -> &'static str {
    match mode {
        themis_core::configuration::ApprovalMode::Yolo => "bypassPermissions",
        // Auto never prompts, like Claude Code's own auto mode.
        themis_core::configuration::ApprovalMode::Auto => "auto",
        themis_core::configuration::ApprovalMode::Custom => "default",
    }
}

/// The server's own tool name inside an MCP wire name (`mcp_<server>_<16 hex>_<tool>`).
pub(super) fn mcp_remote_name(wire: &str) -> Option<&str> {
    let server = themis_core::tools::mcp_server_id(wire)?;
    wire.strip_prefix("mcp_")?
        .get(server.len()..)?
        .strip_prefix('_')
        .filter(|remote| !remote.is_empty())
}

/// Why an MCP connection of a run failed to start, as a fixed category: transport errors may
/// contain remote URLs, arguments or server responses, so only actionable categories show.
pub(super) fn mcp_failure_reason(error: &anyhow::Error) -> &'static str {
    let text = error.to_string();
    let missing = format!("{error:#}").contains("MCP program not found")
        || text.starts_with("MCP process could not start: ");
    match text.as_str() {
        _ if missing => "server process could not start: its program was not found",
        "Could not start MCP server" | "MCP process could not start. Check the saved command." => {
            "server process could not start"
        }
        "Required MCP environment variable is unset"
        | "MCP authentication variable is unset"
        | "Required MCP environment variable is unavailable." => {
            "required authentication or environment variable is unavailable"
        }
        "MCP startup denied" => "startup approval was denied",
        "MCP startup needs your approval" => {
            "it has not been enabled by you since its definition last changed"
        }
        "Unsupported MCP protocol version" => "server protocol is unsupported",
        _ => "connection or tool discovery failed",
    }
}
