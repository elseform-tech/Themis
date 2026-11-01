//! Per-chat approval grants ("Allow in this chat").
//!
//! A grant is the same command family an approval prompt describes (see
//! [`ToolAction::grant_key`]): `cargo test` covers `cargo test --lib`, an MCP
//! grant covers every tool of that server. Grants persist in a small versioned
//! JSON file owned by the chat so later runs in the same chat inherit them.
//!
//! The file is untrusted input. Anything that does not validate is ignored as a
//! whole, so a damaged or hand-edited file can only remove permissions, never
//! add them. Destructive actions are never stored and never matched.
use crate::tools::{RiskLevel, ToolAction};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Format version written to and required from the grant file.
pub const GRANTS_VERSION: u32 = 1;
/// Largest grant file read; bigger files are ignored.
pub const MAX_FILE_BYTES: u64 = 256 * 1024;
/// Most grants a chat keeps.
pub const MAX_GRANTS: usize = 256;
const MAX_TOKENS: usize = 64;
const MAX_TOKEN_BYTES: usize = 2048;
const MAX_NAME_BYTES: usize = 256;

/// One stored grant. Exactly the data [`ToolAction::grant_key`] derives its key
/// from, plus the risk class the grant was made for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantEntry {
    /// Tool name (`shell`, `git`, `write_file`, ...). For a server-wide grant
    /// this is the fixed word `mcp`.
    pub tool: String,
    /// Approved argv prefix for `shell`, `git` and `shell_unsandboxed`; empty
    /// for every other tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    /// Server identity for a grant covering all tools of one MCP server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// `read`, `write`, `execute` or `network`; never `destructive`.
    pub risk: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantFile {
    version: u32,
    grants: Vec<GrantEntry>,
}

fn risk_label(risk: RiskLevel) -> &'static str {
    match risk {
        RiskLevel::Read => "read",
        RiskLevel::Write => "write",
        RiskLevel::Execute => "execute",
        RiskLevel::Network => "network",
        RiskLevel::Destructive => "destructive",
    }
}

fn clean(text: &str, limit: usize) -> bool {
    !text.is_empty() && text.len() <= limit && !text.chars().any(char::is_control)
}

impl GrantEntry {
    /// The entry that records a grant for `action`, or `None` when the action
    /// can never be granted (destructive, read-only, or unscoped shell).
    pub fn for_action(action: &ToolAction) -> Option<Self> {
        if !action.grantable() {
            return None;
        }
        let entry = if let Some(server) = crate::tools::mcp_server_id(&action.tool) {
            Self {
                tool: "mcp".to_owned(),
                command: Vec::new(),
                server: Some(server.to_owned()),
                risk: risk_label(action.risk).to_owned(),
            }
        } else if let Some((group, site)) = action.site_grant() {
            Self {
                tool: group,
                command: vec![site],
                server: None,
                risk: risk_label(action.risk).to_owned(),
            }
        } else if action.tool == crate::tools::sensitive::SENSITIVE_TOOL {
            Self {
                tool: action.tool.clone(),
                command: action.command.clone(),
                server: None,
                risk: risk_label(action.risk).to_owned(),
            }
        } else if crate::tools::is_command_tool(&action.tool) {
            let scope = action.grant_scope_argv()?;
            Self {
                tool: action.tool.clone(),
                command: scope,
                server: None,
                risk: risk_label(action.risk).to_owned(),
            }
        } else {
            Self {
                tool: action.tool.clone(),
                command: Vec::new(),
                server: None,
                risk: risk_label(action.risk).to_owned(),
            }
        };
        entry.validate().is_ok().then_some(entry)
    }

    /// The key [`ToolAction::grant_key`] produces for actions this entry covers.
    pub fn key(&self) -> String {
        if let Some(server) = &self.server {
            return format!("mcp\0{server}");
        }
        if self.command.is_empty() {
            return self.tool.clone();
        }
        format!("{}\0{}", self.tool, self.command.join("\0"))
    }

    /// Structural and semantic validation shared by loading and recording.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !clean(&self.tool, MAX_NAME_BYTES) {
            return Err("invalid tool name");
        }
        if !matches!(self.risk.as_str(), "read" | "write" | "execute" | "network") {
            return Err("risk is not grantable");
        }
        if self.command.len() > MAX_TOKENS
            || self.command.iter().any(|token| {
                token.is_empty() || token.len() > MAX_TOKEN_BYTES || token.contains('\0')
            })
        {
            return Err("invalid command");
        }
        match (&self.server, self.tool.as_str()) {
            (Some(server), "mcp") => {
                if !clean(server, MAX_NAME_BYTES) || !self.command.is_empty() {
                    return Err("invalid server grant");
                }
            }
            (Some(_), _) => return Err("server is only valid for mcp"),
            (None, "mcp") => return Err("mcp grant needs a server"),
            (None, "shell" | "git" | crate::tools::UNSANDBOXED_TOOL) => {
                if self.command.is_empty() {
                    return Err("command grant needs a command");
                }
                // A stored command must stay exactly as safe as a live grant.
                if crate::tools::is_destructive_variant(&self.command) {
                    return Err("destructive command");
                }
            }
            (None, crate::tools::sensitive::SENSITIVE_TOOL) => {
                // One kind of access to one file.
                let [access, _path] = self.command.as_slice() else {
                    return Err("sensitive file grant needs an access and a path");
                };
                if !matches!(access.as_str(), "read" | "write" | "commit") {
                    return Err("invalid sensitive file access");
                }
            }
            (None, "browser" | "fetch_url" | "web_search") => {
                // A site grant names exactly one host.
                let [site] = self.command.as_slice() else {
                    return Err("site grant needs one site");
                };
                if !site
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || ".-:[]".contains(c))
                {
                    return Err("invalid site");
                }
            }
            (None, _) => {
                if !self.command.is_empty() {
                    return Err("command is only valid for shell, git and shell_unsandboxed");
                }
            }
        }
        Ok(())
    }
}

/// Parses and validates grant-file text. Any defect rejects the whole file.
pub fn parse_grants(text: &str) -> Result<Vec<GrantEntry>, &'static str> {
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err("grant file too large");
    }
    let file: GrantFile = serde_json::from_str(text).map_err(|_| "grant file is not valid")?;
    if file.version != GRANTS_VERSION {
        return Err("unsupported grant file version");
    }
    if file.grants.len() > MAX_GRANTS {
        return Err("too many grants");
    }
    let mut seen = std::collections::HashSet::new();
    let mut entries = Vec::new();
    for entry in file.grants {
        entry.validate()?;
        if seen.insert(entry.clone()) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// A chat's grants: loaded when a run starts, extended during the run, and
/// written back so later runs in the same chat inherit them.
pub struct ChatGrants {
    path: Option<PathBuf>,
    entries: Mutex<Vec<GrantEntry>>,
    warning: Option<&'static str>,
}

impl std::fmt::Debug for ChatGrants {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatGrants")
            .field("count", &self.len())
            .finish_non_exhaustive()
    }
}

impl ChatGrants {
    /// Grants held in memory only (no chat storage).
    pub fn in_memory() -> Arc<Self> {
        Arc::new(Self {
            path: None,
            entries: Mutex::new(Vec::new()),
            warning: None,
        })
    }

    /// Loads the chat's grant file. A missing file means no grants; an
    /// unreadable or invalid one is ignored (see [`Self::warning`]) and the
    /// next saved grant replaces it.
    pub fn load(path: &Path) -> Arc<Self> {
        let (entries, warning) = match read_limited(path) {
            Ok(None) => (Vec::new(), None),
            Ok(Some(text)) => match parse_grants(&text) {
                Ok(entries) => (entries, None),
                Err(reason) => (Vec::new(), Some(reason)),
            },
            Err(reason) => (Vec::new(), Some(reason)),
        };
        Arc::new(Self {
            path: Some(path.to_owned()),
            entries: Mutex::new(entries),
            warning,
        })
    }

    /// Why the stored file was ignored at load time, as fixed text.
    pub fn warning(&self) -> Option<&'static str> {
        self.warning
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The stored grants, in file order.
    pub fn entries(&self) -> Vec<GrantEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Whether a stored grant covers `action`. Destructive actions never match.
    pub fn covers(&self, action: &ToolAction) -> bool {
        if !action.grantable() {
            return false;
        }
        let key = action.grant_key();
        let risk = risk_label(action.risk);
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|entry| entry.risk == risk && entry.key() == key)
    }

    /// Records a grant for `action` and writes the chat file. Returns `false`
    /// (and stores nothing) when the action cannot be granted.
    pub fn remember(&self, action: &ToolAction) -> bool {
        let Some(entry) = GrantEntry::for_action(action) else {
            return false;
        };
        // One writer at a time, merging with what other runs saved meanwhile so
        // concurrent runs never lose each other's grants.
        let _writing = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(path) = &self.path {
            if let Ok(Some(text)) = read_limited(path) {
                for stored in parse_grants(&text).unwrap_or_default() {
                    if !entries.contains(&stored) {
                        entries.push(stored);
                    }
                }
            }
        }
        if !entries.contains(&entry) {
            if entries.len() >= MAX_GRANTS {
                // Keep the grant for this run; the file stays within bounds.
                entries.remove(0);
            }
            entries.push(entry);
        }
        if let Some(path) = &self.path {
            let _ = write_grants(path, &entries);
        }
        true
    }
}

/// Serializes grant and consent file writes within the process.
pub static WRITE_LOCK: Mutex<()> = Mutex::new(());

fn read_limited(path: &Path) -> Result<Option<String>, &'static str> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("grant file is unreadable"),
    };
    let mut text = String::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|_| "grant file is unreadable")?;
    Ok(Some(text))
}

/// Writes the grant file atomically (temporary file, then rename).
pub fn write_grants(path: &Path, entries: &[GrantEntry]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&GrantFile {
        version: GRANTS_VERSION,
        grants: entries.to_vec(),
    })
    .map_err(std::io::Error::other)?;
    write_atomic(path, &text)
}

/// Writes `text` to `path` through a uniquely named temporary file and a rename,
/// so concurrent writers never share a temporary file or leave a torn file.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn site_action(tool: &str, host: &str) -> ToolAction {
        ToolAction {
            display: None,
            command: vec![host.into()],
            tool: tool.into(),
            summary: String::new(),
            risk: RiskLevel::Network,
        }
    }

    #[test]
    fn a_site_grant_covers_that_site_for_its_tool_group_and_survives_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grants.json");
        let grants = ChatGrants::load(&path);
        assert!(grants.remember(&site_action("browser_click", "LocalHost")));
        let reloaded = ChatGrants::load(&path);
        for grants in [&grants, &reloaded] {
            assert!(grants.covers(&site_action("browser_type", "localhost")));
            assert!(grants.covers(&site_action("browser_open", "localhost")));
            assert!(!grants.covers(&site_action("browser_open", "example.com")));
            assert!(!grants.covers(&site_action("fetch_url", "localhost")));
        }
        assert!(grants.remember(&site_action("fetch_url", "docs.rs")));
        assert!(grants.covers(&site_action("fetch_url", "docs.rs")));
        assert!(!grants.covers(&site_action("web_search", "docs.rs")));
        let bad = r#"{"version":1,"grants":[{"tool":"browser","command":["a.com","b.com"],"risk":"network"}]}"#;
        assert!(parse_grants(bad).is_err());
        let bad =
            r#"{"version":1,"grants":[{"tool":"browser","command":["A B"],"risk":"network"}]}"#;
        assert!(parse_grants(bad).is_err());
    }

    fn shell(argv: &[&str]) -> ToolAction {
        ToolAction {
            display: None,
            command: argv.iter().map(|s| s.to_string()).collect(),
            tool: "shell".into(),
            summary: String::new(),
            risk: RiskLevel::Execute,
        }
    }

    /// A connected MCP tool of `server` (16 hex digits) as the runtime names it.
    fn mcp(server: &str, tool: &str) -> ToolAction {
        ToolAction {
            display: None,
            command: Vec::new(),
            tool: format!("mcp_{server}{:08x}_{tool}", tool.len()),
            summary: String::new(),
            risk: RiskLevel::Network,
        }
    }

    #[test]
    fn grant_covers_the_command_family_and_nothing_else() {
        let grants = ChatGrants::in_memory();
        assert!(grants.remember(&shell(&["cargo", "test"])));
        assert!(grants.covers(&shell(&["cargo", "test", "--lib"])));
        assert!(!grants.covers(&shell(&["cargo", "run"])));
        assert!(!grants.covers(&shell(&["rm", "-rf", "x"])));
    }

    #[test]
    fn running_outside_the_sandbox_is_its_own_grant() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.json");
        let grants = ChatGrants::load(&path);
        let mut outside = shell(&["npm", "install"]);
        outside.tool = crate::tools::UNSANDBOXED_TOOL.into();
        assert!(grants.remember(&outside));
        assert!(grants.covers(&outside));
        // Approving the escape does not approve the sandboxed command, or vice versa.
        assert!(!grants.covers(&shell(&["npm", "install"])));
        let reloaded = ChatGrants::load(&path);
        assert!(reloaded.covers(&outside));
        assert_eq!(reloaded.entries()[0].tool, "shell_unsandboxed");
        let mut destructive = shell(&["rm", "-rf", "/x"]);
        destructive.tool = crate::tools::UNSANDBOXED_TOOL.into();
        assert!(!grants.remember(&destructive));
        assert!(parse_grants(r#"{"version":1,"grants":[{"tool":"shell_unsandboxed","command":["rm","-rf","x"],"risk":"execute"}]}"#).is_err());
        assert!(parse_grants(
            r#"{"version":1,"grants":[{"tool":"shell_unsandboxed","command":[],"risk":"execute"}]}"#
        )
        .is_err());
    }

    #[test]
    fn destructive_actions_are_never_stored_or_matched() {
        let grants = ChatGrants::in_memory();
        let force = shell(&["git", "push", "--force"]);
        assert!(!force.grantable());
        assert!(!grants.remember(&force));
        let mut delete = shell(&["x"]);
        delete.tool = "delete_file".into();
        delete.risk = RiskLevel::Destructive;
        assert!(!grants.remember(&delete));
        assert!(grants.is_empty());
        // A hand-written entry cannot make a destructive variant matchable either.
        let forged = GrantEntry {
            tool: "shell".into(),
            command: vec!["git".into(), "push".into(), "--force".into()],
            server: None,
            risk: "execute".into(),
        };
        assert!(forged.validate().is_err());
    }

    #[test]
    fn enabling_a_command_mcp_server_is_never_grantable() {
        let mut action = shell(&["sh", "-c", "exit 0"]);
        action.command.clear();
        action.tool = crate::tools::MCP_COMMAND_APPROVAL_TOOL.into();
        assert!(!action.grantable());
        let grants = ChatGrants::in_memory();
        assert!(!grants.remember(&action));
        assert!(!grants.covers(&action));
    }

    #[test]
    fn server_grant_covers_every_tool_of_that_server_only() {
        let grants = ChatGrants::in_memory();
        assert!(grants.remember(&mcp("00112233445566aa", "one")));
        assert!(grants.covers(&mcp("00112233445566aa", "two")));
        assert!(!grants.covers(&mcp("00112233445566bb", "two")));
        // Startup actions and lookalike names belong to no server.
        let mut start = mcp("00112233445566aa", "two");
        start.tool = "mcp_start_g--docs--1--srv".into();
        assert!(!grants.covers(&start));
    }

    #[test]
    fn risk_class_must_match() {
        let grants = ChatGrants::in_memory();
        assert!(grants.remember(&shell(&["echo", "x"])));
        let mut write = shell(&["echo", "x"]);
        write.risk = RiskLevel::Write;
        assert!(!grants.covers(&write));
    }

    #[test]
    fn grants_round_trip_through_the_chat_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grants").join("chat.json");
        let first = ChatGrants::load(&path);
        assert!(first.is_empty() && first.warning().is_none());
        first.remember(&shell(&["cargo", "test"]));
        first.remember(&mcp("00112233445566aa", "x"));
        let second = ChatGrants::load(&path);
        assert_eq!(second.len(), 2);
        assert!(second.covers(&shell(&["cargo", "test", "-q"])));
        assert!(second.covers(&mcp("00112233445566aa", "y")));
        assert!(
            !ChatGrants::load(&dir.path().join("other.json")).covers(&shell(&["cargo", "test"]))
        );
    }

    #[test]
    fn invalid_files_are_ignored_whole_and_can_be_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.json");
        let valid =
            r#"{"version":1,"grants":[{"tool":"shell","command":["echo","x"],"risk":"execute"}]}"#;
        for bad in [
            "not json",
            r#"{"version":2,"grants":[]}"#,
            r#"{"version":1,"grants":[],"extra":1}"#,
            // One bad entry rejects the file, including its valid siblings.
            r#"{"version":1,"grants":[{"tool":"shell","command":["echo"],"risk":"execute"},{"tool":"shell","command":["git","push","-f"],"risk":"execute"}]}"#,
            r#"{"version":1,"grants":[{"tool":"delete_file","risk":"destructive"}]}"#,
            r#"{"version":1,"grants":[{"tool":"shell","risk":"execute"}]}"#,
            r#"{"version":1,"grants":[{"tool":"read_file","risk":"destructive"}]}"#,
        ] {
            std::fs::write(&path, bad).unwrap();
            let grants = ChatGrants::load(&path);
            assert!(grants.is_empty(), "{bad}");
            assert!(grants.warning().is_some(), "{bad}");
        }
        std::fs::write(&path, valid).unwrap();
        assert_eq!(ChatGrants::load(&path).len(), 1);
        std::fs::write(&path, "garbage").unwrap();
        let grants = ChatGrants::load(&path);
        grants.remember(&shell(&["echo", "y"]));
        assert_eq!(ChatGrants::load(&path).len(), 1);
    }

    #[test]
    fn oversized_files_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.json");
        std::fs::write(&path, " ".repeat(MAX_FILE_BYTES as usize + 8)).unwrap();
        assert!(ChatGrants::load(&path).warning().is_some());
    }

    #[test]
    fn concurrent_runs_of_a_chat_do_not_lose_each_others_grants() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.json");
        // Every writer loaded the (empty) file before any of the others saved.
        let writers: Vec<_> = (0..8).map(|_| ChatGrants::load(&path)).collect();
        std::thread::scope(|scope| {
            for (index, grants) in writers.iter().enumerate() {
                scope.spawn(move || {
                    assert!(grants.remember(&shell(&["tool", &index.to_string()])));
                });
            }
        });
        let saved = ChatGrants::load(&path);
        assert_eq!(saved.len(), 8);
        assert!(saved.warning().is_none());
        let leftovers = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(leftovers, 1, "no temporary files remain");
    }

    #[test]
    fn grant_count_is_bounded() {
        let grants = ChatGrants::in_memory();
        for index in 0..MAX_GRANTS + 5 {
            grants.remember(&shell(&["tool", &index.to_string()]));
        }
        assert_eq!(grants.len(), MAX_GRANTS);
    }

    proptest! {
        #[test]
        fn parser_never_panics_and_only_accepts_safe_entries(text in ".{0,400}") {
            if let Ok(entries) = parse_grants(&text) {
                for entry in entries {
                    prop_assert!(entry.validate().is_ok());
                    prop_assert!(entry.risk != "destructive");
                }
            }
        }

        #[test]
        fn structured_inputs_never_widen_permissions(
            tool in "[a-z_]{0,12}",
            command in proptest::collection::vec("[ -~]{0,8}", 0..5),
            server in proptest::option::of("[a-z-]{0,10}"),
            risk in prop_oneof!["read","write","execute","network","destructive","x"],
        ) {
            let text = serde_json::json!({"version":1,"grants":[{"tool":tool,"command":command,"server":server,"risk":risk}]}).to_string();
            if let Ok(entries) = parse_grants(&text) {
                let grants = ChatGrants { path: None, entries: Mutex::new(entries), warning: None };
                // Whatever parsed can never match a destructive action.
                let forced = shell(&["git", "push", "--force", "origin"]);
                prop_assert!(!grants.covers(&forced));
                let mut delete = shell(&[]);
                delete.tool = tool.clone();
                delete.risk = RiskLevel::Destructive;
                prop_assert!(!grants.covers(&delete));
            }
        }

        #[test]
        fn server_id_is_stable_per_namespace_and_never_panics(
            namespace in ".{0,20}", other in ".{0,20}", tool in ".{0,40}", junk in ".{0,60}",
        ) {
            use crate::plugins::connections::tool_identity;
            let name = |ns: &str, t: &str| format!("mcp_{}", tool_identity(ns, t));
            let mine = name(&namespace, &tool);
            let id = crate::tools::mcp_server_id(&mine);
            prop_assert!(id.is_some());
            // Every tool of one server shares the id; another server differs.
            let sibling = name(&namespace, "other");
            prop_assert_eq!(id, crate::tools::mcp_server_id(&sibling));
            if namespace != other {
                let foreign = name(&other, &tool);
                prop_assert_ne!(id, crate::tools::mcp_server_id(&foreign));
            }
            let _ = crate::tools::mcp_server_id(&junk);
        }

        #[test]
        fn recorded_grants_round_trip_and_cover_their_action(
            command in proptest::collection::vec("[a-z0-9./=-]{1,8}", 1..5),
        ) {
            let action = shell(&command.iter().map(String::as_str).collect::<Vec<_>>());
            let grants = ChatGrants::in_memory();
            if grants.remember(&action) {
                prop_assert!(grants.covers(&action));
                let entries = grants.entries.lock().unwrap().clone();
                let text = serde_json::to_string(&GrantFile { version: 1, grants: entries.clone() }).unwrap();
                prop_assert_eq!(parse_grants(&text).unwrap(), entries);
            }
        }
    }
}
