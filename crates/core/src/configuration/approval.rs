use super::*;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    Yolo,
    /// No prompts: where Custom would ask, a reviewer model decides against the person's own
    /// words, and a block goes back to the agent with its reason (see [`crate::tools::AutoReviewer`]).
    Auto,
    #[default]
    Custom,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    Allow,
    #[default]
    Ask,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRule {
    pub tool: String,
    pub action: PolicyAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
    /// Argv prefix (program, then arguments) a `shell` call must start with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApprovalPolicy {
    pub default: PolicyAction,
    pub rules: Vec<ApprovalRule>,
    /// File-name patterns of sensitive project files, which ask first. Replaces the
    /// defaults when set; `[]` turns it off; a project may only add to it.
    pub sensitive_files: Vec<String>,
    /// The model Auto mode's reviewer uses (`<provider>/<id>` for a model from providers);
    /// unset, the first memory model. User file only: a project may not choose who reviews.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_model: Option<String>,
}
impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self {
            default: PolicyAction::Ask,
            rules: vec![ApprovalRule {
                command: None,
                tool: "*".into(),
                action: PolicyAction::Allow,
                risk: Some("read".into()),
            }],
            sensitive_files: crate::tools::sensitive::DEFAULT_SENSITIVE_FILES
                .iter()
                .map(|pattern| (*pattern).to_owned())
                .collect(),
            auto_model: None,
        }
    }
}
impl ApprovalPolicy {
    /// First matching rule decides. `recall` reads the same saved evidence that
    /// `read_file` can open, so a deny that covers `read_file` also denies it.
    pub fn decision(&self, action: &ToolAction) -> PolicyAction {
        let decision = self.rule_decision(action);
        if action.tool == "recall" && decision != PolicyAction::Deny {
            let mirrored = ToolAction {
                tool: "read_file".into(),
                ..action.clone()
            };
            if self.rule_decision(&mirrored) == PolicyAction::Deny {
                return PolicyAction::Deny;
            }
        }
        decision
    }

    /// The rules as YOLO applies them: a deny rule still denies (the defaults
    /// ship none, so every deny rule is one a person or project wrote), while ask
    /// rules and the default allow, because YOLO never asks.
    #[must_use]
    pub fn yolo(&self) -> Self {
        Self {
            default: PolicyAction::Allow,
            rules: self
                .rules
                .iter()
                .filter(|rule| {
                    rule.action == PolicyAction::Deny || rule.action == PolicyAction::Allow
                })
                .cloned()
                .collect(),
            sensitive_files: self.sensitive_files.clone(),
            auto_model: self.auto_model.clone(),
        }
    }

    /// [`Self::yolo`]'s decision: `Deny` for a matching deny rule, otherwise `Allow`.
    pub fn yolo_decision(&self, action: &ToolAction) -> PolicyAction {
        match self.yolo().decision(action) {
            PolicyAction::Deny => PolicyAction::Deny,
            _ => PolicyAction::Allow,
        }
    }

    fn rule_decision(&self, action: &ToolAction) -> PolicyAction {
        self.rules
            .iter()
            .find(|rule| rule_matches(rule, action))
            .map_or(self.default, |rule| rule.action)
    }
}
/// Whether `rule` applies to `action` (tool name or `*` prefix, command
/// prefix with deny-only widening, risk).
pub(super) fn rule_matches(rule: &ApprovalRule, action: &ToolAction) -> bool {
    let matches = rule
        .tool
        .strip_suffix('*')
        .map_or(rule.tool == action.tool, |prefix| {
            action.tool.starts_with(prefix)
        })
        || deny_crosses_command_tools(rule, action);
    if crate::tools::is_site_tool(&action.tool) {
        // Web and browser rules name a site: the host or any of its subdomains.
        return matches
            && rule.command.as_ref().is_none_or(|sites| {
                action
                    .command
                    .first()
                    .is_some_and(|host| site_matches(host, &sites[0]))
            })
            && rule
                .risk
                .as_ref()
                .is_none_or(|risk| risk == risk_name(action.risk));
    }
    matches
        && match rule.command.as_ref() {
            // A broad allow without `command` is trust in the tool,
            // not in its destructive variants (`git push --force`,
            // `rm -rf`): those need a rule that names them or an
            // explicit `"risk": "destructive"`.
            None => {
                rule.action != PolicyAction::Allow
                    || rule.risk.as_deref() == Some("destructive")
                    || !is_destructive_variant(&action.command)
            }
            Some(prefix) => {
                // A broad allow (`["git","push"]`) must not silently cover
                // a destructive variant (`git push --force`) unless the
                // rule itself names that variant.
                let covers_variant = rule.action != PolicyAction::Allow
                    || !is_destructive_variant(&action.command)
                    || is_destructive_variant(prefix);
                // Deny rules also see commands inside wrappers, so
                // `sh -c "git push --force"` is denied by `["git","push","--force"]`.
                covers_variant
                    && (command_has_prefix(&action.command, prefix)
                        || (rule.action == PolicyAction::Deny
                            && crate::tools::commands_run_by(&action.command)
                                .unwrap_or_else(|| vec![action.command.clone()])
                                .iter()
                                .any(|command| deny_matches(command, prefix))))
            }
        }
        && rule
            .risk
            .as_ref()
            .is_none_or(|risk| risk == risk_name(action.risk))
}
/// `shell` and the `git` tool both run git, so a deny rule on one holds on the other: a rule on
/// `shell` (with or without a command) covers the `git` tool, and a rule on `git` naming a
/// command covers a shell call running it. A rule on `git` with no command stays about the tool.
fn deny_crosses_command_tools(rule: &ApprovalRule, action: &ToolAction) -> bool {
    rule.action == PolicyAction::Deny
        && match (rule.tool.as_str(), action.tool.as_str()) {
            ("shell", "git") => true,
            ("git", "shell") => rule.command.is_some(),
            _ => false,
        }
}
/// `host` is `site` or one of its subdomains (`docs.example.com` for
/// `example.com`), compared without case.
pub fn site_matches(host: &str, site: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let site = site.trim_end_matches('.').to_ascii_lowercase();
    !site.is_empty()
        && (host == site
            || host
                .strip_suffix(&site)
                .is_some_and(|rest| rest.ends_with('.')))
}

/// A site in an approval rule: a host name or IP address, without scheme,
/// port, path or wildcard.
pub(super) fn valid_site(site: &str) -> bool {
    !site.is_empty()
        && site.len() <= 253
        && site
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
        && !site.starts_with('.')
}
/// Exact token-prefix match: `["cargo","test"]` matches `cargo test --lib` but
/// not `cargo testing`, `/usr/bin/cargo test` or an empty command.
pub fn command_has_prefix(command: &[String], prefix: &[String]) -> bool {
    !prefix.is_empty() && command.len() >= prefix.len() && command[..prefix.len()] == *prefix
}
/// Normalized program name: basename, lowercased on macOS/Windows
/// (case-insensitive file systems), `.exe`/`.cmd`/`.bat` stripped on Windows.
fn normalize_program(program: &str) -> String {
    let base = program
        .rsplit(|c| c == '/' || (cfg!(windows) && c == '\\'))
        .next()
        .unwrap_or(program);
    let mut name = if cfg!(any(target_os = "macos", windows)) {
        base.to_lowercase()
    } else {
        base.to_owned()
    };
    if cfg!(windows) {
        for ext in [".exe", ".cmd", ".bat"] {
            if let Some(stem) = name.strip_suffix(ext) {
                name = stem.to_owned();
                break;
            }
        }
    }
    name
}
/// Deny-rule match: the program token is normalized on both sides (so `/bin/rm`,
/// `./rm` and `RM` on case-insensitive systems are still denied by `["rm"]`) and
/// flags are compared by meaning rather than spelling (see [`flag_present`]):
/// the rule's non-flag tokens must begin the command's non-flag tokens, and each
/// flag the rule names must appear anywhere in the command before `--`. So
/// `["git","push","--force"]` also denies `git push -f origin main` and
/// `git push origin --force`, and `["rm","-rf"]` denies `rm -fr x` and
/// `rm -r -f x`. Deny rules may only widen this way; allow/ask rules stay exact.
pub(super) fn deny_matches(command: &[String], prefix: &[String]) -> bool {
    let (Some((program, rest)), Some((first, wanted))) =
        (command.split_first(), prefix.split_first())
    else {
        return false;
    };
    let program = normalize_program(program);
    if program != normalize_program(first) {
        return false;
    }
    // `git -C . push` is still a push.
    let rest = if program == "git" && !wanted.first().is_some_and(|word| word.starts_with('-')) {
        crate::tools::git_from_subcommand(rest)
    } else {
        rest
    };
    if rest.starts_with(wanted) {
        return true;
    }
    let before_separator: Vec<&str> = rest
        .iter()
        .map(String::as_str)
        .take_while(|arg| *arg != "--")
        .collect();
    let is_flag = |token: &str| token.len() > 1 && token.starts_with('-');
    let mut words = before_separator.iter().filter(|arg| !is_flag(arg));
    wanted
        .iter()
        .filter(|token| !is_flag(token))
        .all(|token| words.next() == Some(&token.as_str()))
        && wanted
            .iter()
            .filter(|token| is_flag(token))
            .all(|flag| flag_present(&program, flag, &before_separator))
}

/// Short/long spellings with one meaning for a program, used only to widen deny
/// rules: `(program, long name, short letters)`.
const FLAG_EQUIVALENTS: &[(&str, &str, &str)] = &[
    ("git", "force", "f"),
    ("git", "delete", "d"),
    ("git", "all", "a"),
    ("git", "recursive", "r"),
    ("rm", "force", "f"),
    ("rm", "recursive", "rR"),
    ("rm", "dir", "d"),
    ("cp", "force", "f"),
    ("cp", "recursive", "rR"),
    ("mv", "force", "f"),
    ("chmod", "recursive", "R"),
    ("chown", "recursive", "R"),
];

/// Whether the command (tokens before `--`) carries `flag` under any equivalent
/// spelling: the exact token, `--name=value` for `--name`, a short-flag cluster
/// containing every letter of a short rule flag (`-rf` covers `-f`, `-fr`,
/// `-r -f`), or the short/long equivalent from [`FLAG_EQUIVALENTS`].
fn flag_present(program: &str, flag: &str, args: &[&str]) -> bool {
    let shorts: String = args
        .iter()
        .filter(|arg| arg.starts_with('-') && !arg.starts_with("--"))
        .flat_map(|arg| arg.chars().skip(1))
        .collect();
    let has_long = |name: &str| {
        args.iter().any(|arg| {
            arg.strip_prefix("--")
                .is_some_and(|long| long == name || long.starts_with(&format!("{name}=")))
        })
    };
    if args.contains(&flag) {
        return true;
    }
    if let Some(long) = flag.strip_prefix("--") {
        let name = long.split('=').next().unwrap_or(long);
        if long == name && has_long(name) {
            return true;
        }
        return long == name
            && FLAG_EQUIVALENTS
                .iter()
                .filter(|(p, l, _)| *p == program && *l == name)
                .any(|(_, _, letters)| letters.chars().any(|c| shorts.contains(c)));
    }
    let letters = &flag[1..];
    !letters.is_empty()
        && letters.chars().all(|letter| {
            shorts.contains(letter)
                || FLAG_EQUIVALENTS
                    .iter()
                    .filter(|(p, _, short)| *p == program && short.contains(letter))
                    .any(|(_, long, short)| {
                        has_long(long) || short.chars().any(|c| shorts.contains(c))
                    })
        })
}
fn risk_name(risk: RiskLevel) -> &'static str {
    match risk {
        RiskLevel::Read => "read",
        RiskLevel::Write => "write",
        RiskLevel::Execute => "execute",
        RiskLevel::Network => "network",
        RiskLevel::Destructive => "destructive",
    }
}
