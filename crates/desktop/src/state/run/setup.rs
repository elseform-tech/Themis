//! Run setup helpers: the project secrets note, the hooks' permission mode and MCP tool names.

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
