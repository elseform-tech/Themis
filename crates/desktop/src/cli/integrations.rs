//! `themis doctor --integrations`: checks every installed and custom plugin, skill, MCP server
//! and hook, global and in the project, and reports every problem with its item, scope and a
//! one-line fix.
//!
//! It reads the store through the app server (`plugin_action` `list`, a read an agent's
//! credential already has) and checks the rest here, in the `themis` process: each plugin's
//! definition against the store's validation, each skill's file, each hook's commands found (never
//! run). Each switched-on MCP server is started and its tools listed, within the connection's
//! 30-second start limit: for you through the app server's `test_mcp`, as `themis mcp test` does;
//! in an agent's shell here, inside the run's sandbox, because the server never starts a command
//! for an agent's credential. `--no-start` leaves the servers alone, and is then only a read.

use std::path::{Path, PathBuf};

use futures_util::{stream, StreamExt};
use serde::Serialize;
use serde_json::{json, Value};
use themis_core::plugins::{Hook, PluginSpec};

use crate::server::Client;

/// One problem, or a server that was not started: what, where, and how to fix it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    /// `plugin`, `skill`, `hook` or `mcp`.
    pub kind: &'static str,
    /// The item: `plugin/name`, or the plugin's name for the plugin itself.
    pub item: String,
    pub scope: String,
    pub problem: String,
    pub fix: String,
}

/// What `doctor --integrations` found.
#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub ok: bool,
    pub checked: Checked,
    pub problems: Vec<Finding>,
    /// Servers it did not start (switched off, `--no-start`, or needing a secret in an agent's
    /// shell): not problems, listed so the report says what it did not check.
    pub not_started: Vec<Finding>,
}

/// How many of each it looked at.
#[derive(Debug, Default, Serialize)]
pub struct Checked {
    pub plugins: usize,
    pub skills: usize,
    pub hooks: usize,
    pub mcp_servers: usize,
    pub mcp_started: usize,
}

/// `doctor --integrations [--json] [--no-start] [--project PATH]`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub json: bool,
    pub start: bool,
    pub project: Option<PathBuf>,
}

impl Options {
    /// The words after `doctor`; `None` when they are not an integrations check.
    #[must_use]
    pub fn parse(words: &[String]) -> Option<Self> {
        let mut options = Self {
            start: true,
            ..Self::default()
        };
        let mut integrations = false;
        let mut index = 0;
        while let Some(word) = words.get(index) {
            match word.as_str() {
                "--integrations" => integrations = true,
                "--json" => options.json = true,
                "--no-start" => options.start = false,
                "--project" => {
                    index += 1;
                    options.project = Some(PathBuf::from(words.get(index)?));
                }
                _ => return None,
            }
            index += 1;
        }
        integrations.then_some(options)
    }
}

/// Servers start four at a time, each within its own start limit.
const STARTING_AT_ONCE: usize = 4;
/// The longest part of an error the report keeps.
const ERROR_CHARS: usize = 400;

/// Runs every check. Every integration is global; `--project` (else the current folder) is
/// where a hook's relative command runs from.
pub async fn run(client: &Client, directory: &Path, options: &Options) -> Result<Report, String> {
    let here = std::env::current_dir().map_err(|error| error.to_string())?;
    let project = options.project.clone();
    let list = client
        .call(
            "plugin_action",
            json!({"action": "list", "projectRoot": project}),
        )
        .await?;
    let mut report = Report::default();
    let mut servers = Vec::new();
    for plugin in list.as_array().ok_or("Invalid plugin list")? {
        // Skills found on disk are not installed: their folder is theirs.
        if plugin["source"] == "discovered" {
            continue;
        }
        let stores = Stores {
            data: directory,
            project: project.as_deref(),
        };
        servers.extend(check_plugin(plugin, &stores, &mut report));
    }
    let root = project.unwrap_or(here);
    start_servers(client, servers, &root, options.start, &mut report).await;
    report.ok = report.problems.is_empty();
    Ok(report)
}

/// Where the plugins keep their packages (only the global store has any), and the project
/// a hook's relative command is run from.
struct Stores<'a> {
    data: &'a Path,
    project: Option<&'a Path>,
}

impl Stores<'_> {
    fn packages(&self, scope: &str) -> Option<PathBuf> {
        (scope == "global").then(|| self.data.join("plugins").join("packages"))
    }
}

/// A switched-on server to start.
struct Server {
    plugin: String,
    id: String,
    scope: String,
    definition: Value,
}

/// Checks one plugin's definition, skills and hooks; returns its switched-on servers.
fn check_plugin(plugin: &Value, stores: &Stores, report: &mut Report) -> Vec<Server> {
    let scope = plugin["scope"].as_str().unwrap_or("global").to_owned();
    let name = plugin["spec"]["name"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let installed = plugin["source"].as_str().is_some();
    report.checked.plugins += 1;
    let mut found = |kind: &'static str, item: String, problem: String, fix: String| {
        report.problems.push(Finding {
            kind,
            item,
            scope: scope.clone(),
            problem,
            fix,
        });
    };
    for problem in themis_core::plugins::definitions::plugin_problems(&plugin["spec"]) {
        found(
            "plugin",
            name.clone(),
            problem,
            definition_fix(&name, installed),
        );
    }
    let Ok(spec) = serde_json::from_value::<PluginSpec>(plugin["spec"].clone()) else {
        return Vec::new();
    };
    let files = Files::new(&spec, stores.packages(&scope));
    if let Some(problem) = files.missing_package() {
        found(
            "plugin",
            name.clone(),
            problem,
            reinstall_fix(&name, installed),
        );
    }
    for skill in &spec.skills {
        report.checked.skills += 1;
        if let Some(problem) = skill_problem(&spec, &skill.id, &files) {
            let item = format!("{name}/{}", skill.id);
            found("skill", item, problem, reinstall_fix(&name, installed));
        }
    }
    let root = stores.project.unwrap_or(Path::new("."));
    for hook in &spec.hooks {
        report.checked.hooks += 1;
        if let Some(problem) = hook_problem(hook, &files, root) {
            let fix = if installed {
                reinstall_fix(&name, installed)
            } else {
                format!(
                    "Install what it runs, or fix its command: themis hook update {name} {} FILE",
                    hook.name
                )
            };
            found("hook", format!("{name}/{}", hook.name), problem, fix);
        }
    }
    servers(plugin, &spec, &name, &scope, report)
}

/// The plugin's MCP servers: those switched on to start, the rest listed as not started.
fn servers(
    plugin: &Value,
    spec: &PluginSpec,
    name: &str,
    scope: &str,
    report: &mut Report,
) -> Vec<Server> {
    let plugin_on = plugin["enabled"].as_bool().unwrap_or(false);
    let mut start = Vec::new();
    for (id, server) in &spec.mcp {
        report.checked.mcp_servers += 1;
        if plugin_on && server.enabled {
            start.push(Server {
                plugin: name.to_owned(),
                id: id.clone(),
                scope: scope.to_owned(),
                definition: plugin["spec"]["mcp"][id].clone(),
            });
        } else {
            report.not_started.push(Finding {
                kind: "mcp",
                item: format!("{name}/{id}"),
                scope: scope.to_owned(),
                problem: "switched off".into(),
                fix: "The person switches it on in Integrations".into(),
            });
        }
    }
    start
}

fn definition_fix(name: &str, installed: bool) -> String {
    if installed {
        format!("Its definition no longer passes the store's checks: themis plugin update {name}, or reinstall it")
    } else {
        format!("Fix the definition: themis plugin export {name} > FILE, edit FILE, themis plugin save FILE")
    }
}

fn reinstall_fix(name: &str, installed: bool) -> String {
    if installed {
        format!("Reinstall it: themis plugin uninstall {name}, then themis plugin install {name}@MARKETPLACE")
    } else {
        format!("Import it again from its source: themis plugin import SOURCE {name}")
    }
}

/// A plugin's files: inline in the registry, or in its kept package folder.
struct Files<'a> {
    spec: &'a PluginSpec,
    package: Option<PathBuf>,
}

impl<'a> Files<'a> {
    fn new(spec: &'a PluginSpec, packages: Option<PathBuf>) -> Self {
        let package = spec
            .package
            .as_ref()
            .and_then(|kept| Some(packages?.join(&kept.hash)));
        Self { spec, package }
    }

    fn missing_package(&self) -> Option<String> {
        let folder = self.package.as_ref()?;
        (!folder.is_dir()).then(|| {
            "its package folder is missing, so files outside the registry are gone".to_owned()
        })
    }

    fn on_disk(&self, relative: &str) -> Option<PathBuf> {
        Some(self.package.as_ref()?.join(relative))
    }

    fn readable(&self, relative: &str) -> bool {
        self.spec.files.contains_key(relative)
            || self
                .on_disk(relative)
                .is_some_and(|path| std::fs::File::open(path).is_ok())
    }

    fn executable(&self, relative: &str) -> bool {
        if self.spec.files.contains_key(relative) {
            return self
                .spec
                .executable_files
                .iter()
                .any(|file| file == relative);
        }
        self.on_disk(relative)
            .is_some_and(|path| executable_file(&path))
    }
}

/// A skill whose file is missing or unreadable (one saved inline has none to check).
fn skill_problem(spec: &PluginSpec, id: &str, files: &Files) -> Option<String> {
    let path = spec.skill_paths.get(id)?;
    let readable = if Path::new(path).is_absolute() {
        std::fs::File::open(path).is_ok()
    } else {
        files.readable(path)
    };
    (!readable).then(|| format!("its file {path} is missing or unreadable"))
}

/// Shell words that are not programs to find.
const SHELL_WORDS: &[&str] = &[
    ":", ".", "[", "alias", "break", "cd", "command", "continue", "echo", "eval", "exec", "exit",
    "export", "false", "printf", "pwd", "read", "return", "set", "shift", "source", "test", "trap",
    "true", "type", "ulimit", "umask", "unset", "wait",
];

/// Programs that run a script named by their first argument.
const INTERPRETERS: &[&str] = &[
    "bash", "bun", "deno", "node", "perl", "php", "python", "python3", "ruby", "sh", "zsh",
];

/// The first command of a hook that cannot be found, or a script it names that is missing.
/// Nothing is run. A command it cannot read (or one starting with another variable) is passed.
fn hook_problem(hook: &Hook, files: &Files, root: &Path) -> Option<String> {
    if hook.prompt.is_some() || hook.command.trim().is_empty() {
        return None;
    }
    let script = [
        "sh".to_owned(),
        "-c".to_owned(),
        mark_plugin_root(&hook.command),
    ];
    let commands = themis_core::tools::commands_run_by(&script)?;
    commands
        .iter()
        .skip(1)
        .find_map(|command| command_problem(command, files, root))
}

fn command_problem(command: &[String], files: &Files, root: &Path) -> Option<String> {
    let program = command.first()?;
    if let Some(problem) = program_problem(program, files, root) {
        return Some(problem);
    }
    let name = program.rsplit('/').next().unwrap_or(program);
    if !INTERPRETERS.contains(&name) {
        return None;
    }
    let script = command.get(1).filter(|word| !word.starts_with('-'))?;
    let found = match plugin_relative(script) {
        Some(relative) => files.readable(relative),
        None if script.contains('$') => true,
        None => resolve(script, root).is_file(),
    };
    (!found).then(|| format!("the script {} it runs is missing", shown(script)))
}

fn program_problem(program: &str, files: &Files, root: &Path) -> Option<String> {
    if let Some(relative) = plugin_relative(program) {
        return if !files.readable(relative) {
            Some(format!("{} is not in the plugin", shown(program)))
        } else if !files.executable(relative) {
            Some(format!("{} is not executable", shown(program)))
        } else {
            None
        };
    }
    if program.contains('$') || SHELL_WORDS.contains(&program) {
        return None;
    }
    if program.contains('/') {
        let path = resolve(program, root);
        return if !path.is_file() {
            Some(format!("{program} does not exist"))
        } else if !executable_file(&path) {
            Some(format!("{program} is not executable"))
        } else {
            None
        };
    }
    (!on_path(program)).then(|| format!("`{program}` is not found on PATH"))
}

/// What a hook's command calls the plugin's own folder, and the plain word it becomes before the
/// command is read (the shell reader passes over words with variables).
const PLUGIN_ROOT_NAMES: [&str; 4] = [
    "${CLAUDE_PLUGIN_ROOT}",
    "$CLAUDE_PLUGIN_ROOT",
    "${PLUGIN_ROOT}",
    "$PLUGIN_ROOT",
];
const PLUGIN_ROOT_MARK: &str = "@plugin-root@";

fn mark_plugin_root(command: &str) -> String {
    PLUGIN_ROOT_NAMES
        .iter()
        .fold(command.to_owned(), |text, name| {
            text.replace(name, PLUGIN_ROOT_MARK)
        })
}

/// A word as the hook wrote it, for messages.
fn shown(word: &str) -> String {
    word.replace(PLUGIN_ROOT_MARK, "${CLAUDE_PLUGIN_ROOT}")
}

/// `${CLAUDE_PLUGIN_ROOT}/x` (marked): `x` inside the plugin.
fn plugin_relative(word: &str) -> Option<&str> {
    word.strip_prefix(PLUGIN_ROOT_MARK)?.strip_prefix('/')
}

/// A path as the hook's shell sees it: `~` is the home folder, relative is the project's.
fn resolve(path: &str, root: &Path) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => themis_core::sandbox::home_dir()
            .unwrap_or_default()
            .join(rest),
        None => root.join(path),
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|folder| executable_file(&folder.join(program)))
    })
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

/// Starts each switched-on server and lists its tools: through the app server for you, here in
/// the run's sandbox in an agent's shell.
async fn start_servers(
    client: &Client,
    servers: Vec<Server>,
    root: &Path,
    start: bool,
    report: &mut Report,
) {
    if !start {
        report
            .not_started
            .extend(servers.into_iter().map(|server| Finding {
                kind: "mcp",
                item: format!("{}/{}", server.plugin, server.id),
                scope: server.scope,
                problem: "not started (--no-start)".into(),
                fix: "Run themis doctor --integrations without --no-start to start it".into(),
            }));
        return;
    }
    let agent = crate::server::in_agent_shell();
    let outcomes: Vec<(Server, Result<Value, String>)> = stream::iter(servers)
        .map(|server| async move {
            let outcome = if agent {
                crate::state::definition_test::run(
                    "test_mcp",
                    &json!({"server": server.definition}),
                    root,
                    true,
                )
                .await
                .map_err(|error| format!("{error:#}"))
            } else {
                client
                    .call(
                        "plugin_action",
                        json!({"action": "test_mcp", "server": server.definition,
                            "projectRoot": root}),
                    )
                    .await
            };
            (server, outcome)
        })
        .buffer_unordered(STARTING_AT_ONCE)
        .collect()
        .await;
    for (server, outcome) in outcomes {
        record_start(server, outcome, agent, report);
    }
}

fn record_start(server: Server, outcome: Result<Value, String>, agent: bool, report: &mut Report) {
    let item = format!("{}/{}", server.plugin, server.id);
    let error = match outcome {
        Ok(_) => {
            report.checked.mcp_started += 1;
            return;
        }
        Err(error) => error,
    };
    if agent && error.contains("can't read secrets") {
        report.not_started.push(Finding {
            kind: "mcp",
            item,
            scope: server.scope,
            problem: "reads a secret, which a check in an agent's shell never does".into(),
            fix: "The person runs themis doctor --integrations in their own terminal".into(),
        });
        return;
    }
    let first_line = error.lines().next().unwrap_or_default();
    let problem: String = first_line.chars().take(ERROR_CHARS).collect();
    let mut fix = format!(
        "See the whole error with themis mcp test {} {}, then fix it with themis mcp update {} {} FILE",
        server.plugin, server.id, server.plugin, server.id
    );
    if agent {
        fix.push_str("; it started inside this run's sandbox, which may be in the way");
    }
    report.problems.push(Finding {
        kind: "mcp",
        item,
        scope: server.scope,
        problem: format!("did not start or list its tools: {problem}"),
        fix,
    });
}

/// The report as text, one line per finding.
#[must_use]
pub fn render(report: &Report) -> String {
    let checked = &report.checked;
    let mut text = format!(
        "Integrations: {} plugins, {} skills, {} hooks, {} MCP servers ({} started)\n",
        checked.plugins, checked.skills, checked.hooks, checked.mcp_servers, checked.mcp_started
    );
    for finding in &report.problems {
        text.push_str(&format!(
            "FAIL {} {}: {}\n  fix: {}\n",
            finding.kind,
            place_name(finding),
            finding.problem,
            finding.fix
        ));
    }
    for finding in &report.not_started {
        text.push_str(&format!(
            "not started: {} {}: {}\n",
            finding.kind,
            place_name(finding),
            finding.problem
        ));
    }
    if report.ok {
        text.push_str("No problems found.\n");
    }
    text
}

/// An item as the person knows it from Integrations: their own (the `personal` holder, which
/// Integrations hides) as `name (yours)`, a project skill as `name (project)`, anything else as
/// `plugin/name`. The JSON keeps `item` and `scope` as they are.
fn place_name(finding: &Finding) -> String {
    match finding.item.strip_prefix("personal/") {
        Some(name) if finding.scope == "global" => format!("{name} (yours)"),
        _ if finding.item == "personal" && finding.scope == "global" => "yours".to_owned(),
        _ if finding.scope == "local" => format!("{} (project)", finding.item),
        _ => finding.item.clone(),
    }
}

/// `themis doctor …`: the app's own checks, or with `--integrations` every integration's.
pub(super) async fn doctor(
    client: &Client,
    directory: &Path,
    words: &[String],
) -> Result<(), String> {
    if words.iter().any(|word| word == "--integrations") {
        let options = Options::parse(words).ok_or_else(|| super::usage().to_owned())?;
        super::ensure_server(client, directory).await?;
        let report = run(client, directory, &options).await?;
        if options.json {
            println!(
                "{}",
                serde_json::to_string(&report).map_err(|error| error.to_string())?
            );
        } else {
            print!("{}", render(&report));
        }
        return if report.ok {
            Ok(())
        } else {
            Err("doctor found integration problems".to_owned())
        };
    }
    if words
        .iter()
        .any(|word| !matches!(word.as_str(), "--json" | "--deep"))
    {
        return Err(super::usage().to_owned());
    }
    let report =
        crate::doctor::diagnose(directory, words.iter().any(|word| word == "--deep")).await;
    if words.iter().any(|word| word == "--json") {
        println!(
            "{}",
            serde_json::to_string(&report).map_err(|error| error.to_string())?
        );
    } else {
        println!("Themis {} — {}", report.version, report.data_dir);
        for check in &report.checks {
            println!("{:?} {}: {}", check.status, check.name, check.detail);
            if !check.action.is_empty() {
                println!("  {}", check.action);
            }
        }
    }
    if report.failed() {
        Err("doctor found failures".to_owned())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seen in the 2026-11-10 A/B: agents quoted `mcp personal/notes (global)` to the person,
    /// whose Integrations shows that server under Yours.
    #[test]
    fn the_text_names_items_as_integrations_does() {
        let finding = |item: &str, scope: &str| Finding {
            kind: "mcp",
            item: item.into(),
            scope: scope.into(),
            problem: "switched off".into(),
            fix: String::new(),
        };
        let report = Report {
            not_started: vec![
                finding("personal/notes", "global"),
                finding("kit/broken", "global"),
                finding("discovered-1/checklist", "local"),
            ],
            ..Default::default()
        };
        let text = render(&report);
        assert!(
            text.contains("not started: mcp notes (yours): switched off"),
            "{text}"
        );
        assert!(
            text.contains("not started: mcp kit/broken: switched off"),
            "{text}"
        );
        assert!(text.contains("discovered-1/checklist (project)"), "{text}");
        assert!(
            !text.contains("personal/") && !text.contains("(global)"),
            "{text}"
        );
    }

    fn words(text: &str) -> Vec<String> {
        text.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn options_need_integrations_and_take_only_their_own_flags() {
        let options =
            Options::parse(&words("--integrations --json --no-start --project p")).unwrap();
        assert!(options.json && !options.start);
        assert_eq!(options.project, Some(PathBuf::from("p")));
        assert!(Options::parse(&words("--json")).is_none());
        assert!(Options::parse(&words("--integrations --deep")).is_none());
        assert!(Options::parse(&words("--integrations --project")).is_none());
    }

    fn hook(command: &str) -> Hook {
        serde_json::from_value(json!({"name": "h", "event": "RunStart", "command": command}))
            .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn a_hook_command_is_found_never_run() {
        let spec = PluginSpec {
            name: "kit".into(),
            files: [("bin/check.sh".to_owned(), "exit 0".to_owned())].into(),
            executable_files: vec!["bin/check.sh".into()],
            ..PluginSpec::default()
        };
        let files = Files::new(&spec, None);
        let root = Path::new("/");
        for fine in [
            "true",
            "sh -c 'exit 0'",
            "${CLAUDE_PLUGIN_ROOT}/bin/check.sh --fast",
            "$HOME/bin/anything",
            "echo hi | cat",
        ] {
            assert_eq!(hook_problem(&hook(fine), &files, root), None, "{fine}");
        }
        for (broken, says) in [
            ("no-such-program-for-doctor", "not found on PATH"),
            ("${CLAUDE_PLUGIN_ROOT}/bin/missing.sh", "not in the plugin"),
            ("/no/such/program", "does not exist"),
            ("python3 /no/such/script.py", "script /no/such/script.py"),
            ("echo hi | no-such-program-for-doctor", "not found on PATH"),
        ] {
            let problem = hook_problem(&hook(broken), &files, root).unwrap_or_default();
            assert!(problem.contains(says), "{broken}: {problem}");
        }
    }

    #[test]
    fn a_skill_file_is_found_inline_or_in_the_package() {
        let mut spec = PluginSpec {
            name: "kit".into(),
            files: [("skills/a/SKILL.md".to_owned(), "x".to_owned())].into(),
            ..PluginSpec::default()
        };
        spec.skill_paths
            .insert("a".into(), "skills/a/SKILL.md".into());
        spec.skill_paths
            .insert("b".into(), "skills/b/SKILL.md".into());
        let files = Files::new(&spec, None);
        assert_eq!(skill_problem(&spec, "a", &files), None);
        assert!(skill_problem(&spec, "b", &files).is_some());
        assert_eq!(skill_problem(&spec, "inline", &files), None);
    }
}
