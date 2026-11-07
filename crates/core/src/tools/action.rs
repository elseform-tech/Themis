use super::*;

// ---------------------------------------------------------------------------
// Approval model
// ---------------------------------------------------------------------------

/// Risk classification for a proposed tool action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    /// Reads state without modifying anything.
    Read,
    /// Modifies files or VCS state in expected ways.
    Write,
    /// Executes an external program.
    Execute,
    /// Touches the network.
    Network,
    /// Irreversible or hard-to-reverse (recursive delete, force push, ...).
    Destructive,
}

/// Management tools carry distinct approvals for reads, changes and executable tests.
pub fn integration_risk(name: &str, args: &Value) -> Option<RiskLevel> {
    if !matches!(
        name,
        "list_plugins" | "save_skill" | "manage_integrations" | "manage_automations"
    ) {
        return None;
    }
    let action = match name {
        "save_skill" => "save_skill",
        "list_plugins" => "list",
        _ => args["action"].as_str().unwrap_or("list"),
    };
    Some(match action {
        "list" | "inventory" | "marketplaces" | "sources" | "updates" | "plugin_icon" => {
            RiskLevel::Read
        }
        "catalog"
        | "preview"
        | "preview_source"
        | "check_sources"
        | "check_source"
        | "install"
        | "update"
        | "scan_marketplace"
        | "add_marketplace"
        | "inspect_marketplace"
        | "inspect_repository"
            if name == "manage_integrations" =>
        {
            RiskLevel::Network
        }
        "import_repository" | "preview_repository" => RiskLevel::Network,
        "test_hook" => RiskLevel::Execute,
        "test_mcp" if args["server"]["command"].is_string() => RiskLevel::Execute,
        "test_mcp" => RiskLevel::Network,
        "delete" | "remove_marketplace" | "remove_component" => RiskLevel::Destructive,
        _ => RiskLevel::Write,
    })
}

/// Tools that decide approval themselves inside `execute`, because the answer
/// depends on state the dispatcher does not see. `manage_automations` lets an
/// agent manage its own chat's automations without a prompt and asks for
/// anything else; the dispatcher therefore neither prompts nor arms a permit.
#[must_use]
pub fn tool_approves_itself(name: &str) -> bool {
    // `create_goal` and `update_goal` only change the calling chat's goal and `ask_user` only adds a
    // question to it; `search_memory` only reads the person's own memories (Settings › Memory
    // turns it off); their deny rules apply inside. Web and browser tools
    // approve per site, and only they know the site (the browser's current page).
    // The subagent tools change nothing themselves: each subagent's own actions
    // are approved as they happen.
    matches!(
        name,
        "manage_automations" | "create_goal" | "update_goal" | "ask_user" | "search_memory"
    ) || crate::skills::SUBAGENT_TOOLS.contains(&name)
        || is_site_tool(name)
}

/// Tools whose approval identity is a site: the action's `command` holds the
/// host, rules may name one (`"command": ["example.com"]`, subdomains match)
/// and an "allow in this chat" grant covers that site.
#[must_use]
pub fn is_site_tool(name: &str) -> bool {
    matches!(name, "web_search" | "fetch_url") || name.starts_with("browser_")
}

/// What one site grant covers: every browser action on the site, or one web tool.
fn site_grant_group(tool: &str) -> &str {
    if tool.starts_with("browser_") {
        "browser"
    } else {
        tool
    }
}

pub fn integration_approval_identity(name: &str, args: &Value) -> String {
    if matches!(name, "manage_integrations" | "manage_automations") {
        use std::hash::{Hash, Hasher};
        let action = args["action"].as_str().unwrap_or("list");
        let mut definition = std::collections::hash_map::DefaultHasher::new();
        if matches!(action, "test_hook" | "test_mcp") {
            args.to_string().hash(&mut definition);
        }
        format!(
            "{}_{action}_{:?}_{:016x}",
            name,
            integration_risk(name, args),
            definition.finish()
        )
    } else {
        name.into()
    }
}

/// Structured command shown in approval UI instead of a joined string, so
/// arguments with spaces, newlines or bidi controls cannot masquerade as a
/// different command. Tokens are raw; renderers must treat them as data.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommandDisplay {
    pub argv: Vec<String>,
    /// Working directory the command runs in, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// True for a shell command started as a background task.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
    /// Set when the sandbox blocked this command and the request asks to run
    /// it outside the sandbox: why it was blocked (from the command's output;
    /// render as untrusted text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_blocked: Option<String>,
}

pub(super) fn git_display(args: &[String], root: &std::path::Path) -> CommandDisplay {
    CommandDisplay {
        argv: std::iter::once("git".to_owned())
            .chain(args.iter().cloned())
            .collect(),
        cwd: Some(root.display().to_string()),
        background: false,
        sandbox_blocked: None,
    }
}

/// A single proposed action awaiting an approval decision.
#[derive(Debug, Clone)]
pub struct ToolAction {
    /// Structured command for shell/git approvals; `None` for other tools.
    pub display: Option<CommandDisplay>,
    /// Tool name, e.g. `"shell"`.
    pub tool: String,
    /// One-line human-readable summary shown in approval UI.
    pub summary: String,
    /// Risk classification.
    pub risk: RiskLevel,
    /// Program followed by its arguments for `shell` actions; empty for every
    /// other tool. Used by command-scoped approval rules and grant caching.
    pub command: Vec<String>,
}

/// The server a connected MCP tool belongs to, read from its generated name
/// (`mcp_<server>_<16 hex>_<tool>`, see `plugins::connections::tool_identity`):
/// `<server>_<16 hex>`. One chat grant covers every tool of the server. Startup actions
/// (`mcp_start_*`) and other names have none.
pub fn mcp_server_id(tool: &str) -> Option<&str> {
    let rest = tool.strip_prefix("mcp_")?;
    let (server, tail) = rest.split_once('_')?;
    let hex = tail.get(..16)?;
    let named = !server.is_empty()
        && server.len() <= 16
        && server
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    let hexed = hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    (named && hexed && tail.get(16..)?.starts_with('_')).then(|| &rest[..server.len() + 1 + 16])
}

/// Programs whose first non-flag argument is a true subcommand, so a grant may
/// cover later calls with the same subcommand and different flags/arguments.
const SUBCOMMAND_PROGRAMS: &[&str] = &[
    "cargo", "git", "npm", "pnpm", "yarn", "go", "rustup", "pip", "pip3", "uv", "poetry",
];

/// Subcommands that execute arbitrary programs or scripts; never shared.
const EXEC_SUBCOMMANDS: &[&str] = &[
    "run",
    "run-script",
    "exec",
    "x",
    "dlx",
    "npx",
    "create",
    "tool",
    "generate",
    "shell",
    "install",
    "add",
];

/// Command prefix an "allow in this chat" grant covers. Tokens are raw argv
/// entries; callers must render them as data, never splice them into prose.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GrantScope {
    pub argv: Vec<String>,
    /// True when later arguments after `argv` are covered too.
    pub covers_further_args: bool,
}

impl ToolAction {
    /// Key under which an interactive "allow in this chat" grant is stored.
    ///
    /// Most tools are keyed by name. For `shell` (and the `git` tool, whose
    /// `command` is `["git", args...]`) the key is the approved command:
    /// program plus first argument only for a short list of subcommand-style
    /// CLIs ([`SUBCOMMAND_PROGRAMS`]) whose first argument is a real
    /// subcommand that does not run arbitrary programs. Every other program,
    /// wrapper or interpreter is keyed by its full argv, so approving
    /// `cargo test` covers `cargo test --lib` but not `cargo run`, `rm ...`,
    /// `sudo rm -rf /` or `find . -delete`. Destructive variants
    /// ([`is_destructive_variant`]) are keyed by their full argv too, so
    /// approving `git push` never covers `git push --force`.
    pub fn grant_key(&self) -> String {
        if let Some(server) = mcp_server_id(&self.tool) {
            return format!("mcp\0{server}");
        }
        if let Some(site) = self.site() {
            return format!("{}\0{site}", site_grant_group(&self.tool));
        }
        if self.tool == sensitive::SENSITIVE_TOOL {
            // One file and one kind of access: reading `.env` is not writing it.
            return format!("{}\0{}", self.tool, self.command.join("\0"));
        }
        if !is_command_tool(&self.tool) || self.command.is_empty() {
            return self.tool.clone();
        }
        let (prefix, _) = self.shell_grant_prefix();
        // NUL cannot occur in argv, so the join is unambiguous.
        format!("{}\0{}", self.tool, prefix.join("\0"))
    }

    /// The host a web or browser action acts on, lowercased.
    pub fn site(&self) -> Option<String> {
        is_site_tool(&self.tool)
            .then(|| self.command.first())
            .flatten()
            .map(|host| host.to_ascii_lowercase())
    }

    /// The tool group and host an "allow in this chat" answer covers, for the
    /// approval dialog: `("browser", "localhost")` covers every browser action
    /// on that site.
    pub fn site_grant(&self) -> Option<(String, String)> {
        self.site()
            .map(|site| (site_grant_group(&self.tool).to_owned(), site))
    }

    /// Whether a chat grant may cover this action. Destructive actions (the
    /// `Destructive` risk class and [`is_destructive_variant`] commands) and
    /// `themis` calls that switch on a hook or an MCP server ([`asks_every_time`])
    /// are asked about every time and are never stored or matched by a grant.
    pub fn grantable(&self) -> bool {
        self.risk != RiskLevel::Destructive && !asks_every_time(&self.command)
    }

    /// Command prefix an "allow in this chat" grant covers, and whether it is
    /// a program + subcommand prefix (`true`) rather than the exact argv.
    fn shell_grant_prefix(&self) -> (&[String], bool) {
        let scoped = self.command.len() >= 2
            && SUBCOMMAND_PROGRAMS.contains(&self.command[0].as_str())
            && !self.command[1].starts_with('-')
            && (self.tool == "git" || !EXEC_SUBCOMMANDS.contains(&self.command[1].as_str()))
            && !asks_every_time(&self.command);
        if scoped {
            (&self.command[..2], true)
        } else {
            (&self.command[..], false)
        }
    }

    /// The command prefix a chat grant stores for `shell`/`git` actions.
    pub fn grant_scope_argv(&self) -> Option<Vec<String>> {
        if !is_command_tool(&self.tool) || self.command.is_empty() {
            return None;
        }
        Some(self.shell_grant_prefix().0.to_vec())
    }

    /// What an interactive grant covers, as structured data for the approval
    /// dialog; `None` for tools keyed by name alone. Derived from the same
    /// prefix as [`Self::grant_key`], so the two cannot disagree. A destructive
    /// variant is covered by its exact argv only (`covers_further_args` false).
    pub fn grant_scope(&self) -> Option<GrantScope> {
        if !(self.tool == "shell" || self.tool == UNSANDBOXED_TOOL) || self.command.is_empty() {
            return None;
        }
        let (prefix, scoped) = self.shell_grant_prefix();
        Some(GrantScope {
            argv: prefix.to_vec(),
            covers_further_args: scoped,
        })
    }
}

/// Tools whose grants and rules are keyed by an argv (`shell`, `git` and
/// running a blocked command outside the sandbox).
pub fn is_command_tool(tool: &str) -> bool {
    matches!(tool, "shell" | "git" | UNSANDBOXED_TOOL)
}
