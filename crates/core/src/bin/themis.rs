//! `themis` — headless CLI harness for single agent tasks (Phase 1).
//!
//! Runs one task against a project directory and prints the [`RunEvent`] stream
//! as readable lines:
//!
//! ```text
//! themis run "add a test for the parser" --project ./myrepo --yes
//! ```
//!
//! Argument parsing is hand-rolled (no CLI dependency). Exit codes: 0 on
//! success, 1 when the run or provider setup fails, 2 on usage/config errors.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use themis_core::providers::{ProviderConfig, ProviderKind};
use themis_core::runtime::{run_task_skilled, CachingApprovals, RunEvent};
use themis_core::skills::{materialize_scripts, validate_skill_input, Skill};
use themis_core::tools::{boxed_tools, AllowAllHook, Approval, ApprovalHook, ToolAction};

/// Default cap on LLM round-trips per `run` invocation.
const DEFAULT_MAX_TURNS: usize = 20;

/// Channel capacity for the run event stream (runs emit a handful of events).
const EVENT_CHANNEL_CAPACITY: usize = 1024;

const HELP: &str = "\
themis — headless Themis coding-agent harness (Phase 1)

usage:
  themis run \"task\" --project DIR [options]
  themis --help

run options:
  --project DIR      project root that tools are scoped to (required)
  --provider KIND    go (default: go)
  --model MODEL      model ID (for go: required, or set THEMIS_GO_MODEL;
                     list valid IDs with refresh_go_models)
  --base-url URL     Go endpoint override (test hook)
  --api-key KEY      API key (else THEMIS_GO_API_KEY)
  --yes              approve all tool actions without prompting
  --max-turns N      max agent turns (default: 20)
  --skills-file PATH JSON array of Skill bundles (instructions + tool
                     grants + scripts) applied to this run

exit codes: 0 success, 1 run failure, 2 usage/config error
";

/// Parsed top-level command.
enum Command {
    /// Print help.
    Help,
    /// Run one task.
    Run(RunOptions),
}

/// Parsed `run` options.
struct RunOptions {
    task: String,
    project: PathBuf,
    provider: ProviderKind,
    model: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    yes: bool,
    max_turns: usize,
    skills_file: Option<PathBuf>,
}

/// Stdin approval hook: prompts once/always/deny per action.
///
/// Empty input, EOF, and read errors all deny (safe default).
struct StdinHook;

impl ApprovalHook for StdinHook {
    fn approve(&self, action: &ToolAction) -> Approval {
        eprintln!(
            "approval needed: [{}] {} (risk: {:?})",
            action.tool, action.summary, action.risk
        );
        loop {
            eprint!("allow? [y] once / [a] always / [n] deny (default n): ");
            std::io::stderr().flush().ok();
            let mut line = String::new();
            match std::io::stdin().read_line(&mut line) {
                Ok(0) | Err(_) => return Approval::Deny,
                Ok(_) => match parse_approval_response(&line) {
                    Some(decision) => return decision,
                    None => continue,
                },
            }
        }
    }
}

fn parse_approval_response(line: &str) -> Option<Approval> {
    match line.trim().to_lowercase().as_str() {
        "y" | "yes" | "once" => Some(Approval::AllowOnce),
        "a" | "always" => Some(Approval::AllowAlways),
        "n" | "no" | "deny" | "" => Some(Approval::Deny),
        _ => None,
    }
}

#[tokio::main]
async fn main() {
    std::process::exit(run().await);
}

/// CLI entry point returning the process exit code.
async fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match parse_args(&args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!("run 'themis --help' for usage.");
            return 2;
        }
    };
    match command {
        Command::Help => {
            print!("{HELP}");
            0
        }
        Command::Run(options) => run_task_command(options).await,
    }
}

/// Parses the top-level command line.
fn parse_args(args: &[String]) -> Result<Command, String> {
    if args.is_empty() {
        return Err("no command given.".to_string());
    }
    match args[0].as_str() {
        "-h" | "--help" | "help" => Ok(Command::Help),
        "run" => parse_run(&args[1..]),
        other => Err(format!("unknown command '{other}'.")),
    }
}

/// Parses `run` options; supports `--flag value` and `--flag=value`.
struct PendingRunOptions {
    task: Option<String>,
    project: Option<PathBuf>,
    provider: ProviderKind,
    model: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    yes: bool,
    max_turns: usize,
    skills_file: Option<PathBuf>,
}

impl PendingRunOptions {
    fn new() -> Self {
        Self {
            task: None,
            project: None,
            provider: ProviderKind::Go,
            model: None,
            base_url: None,
            api_key: None,
            yes: false,
            max_turns: DEFAULT_MAX_TURNS,
            skills_file: None,
        }
    }

    fn set_flag(&mut self, flag: &str, value: String) -> Result<(), String> {
        match flag {
            "--project" => self.project = Some(PathBuf::from(value)),
            "--provider" => {
                self.provider = value
                    .parse::<ProviderKind>()
                    .map_err(|err| format!("invalid --provider '{value}': {err:#}"))?;
            }
            "--model" => self.model = Some(value),
            "--base-url" => self.base_url = Some(value),
            "--api-key" => self.api_key = Some(value),
            "--max-turns" => {
                self.max_turns = value.parse().map_err(|_| {
                    format!("invalid --max-turns '{value}': expected a positive integer.")
                })?;
                if self.max_turns == 0 {
                    return Err("invalid --max-turns '0': expected at least 1.".to_string());
                }
            }
            "--skills-file" => self.skills_file = Some(PathBuf::from(value)),
            _ => unreachable!("value flag matched above"),
        }
        Ok(())
    }

    fn finish(self) -> Result<Command, String> {
        let Some(task) = self.task.filter(|task| !task.trim().is_empty()) else {
            return Err("missing task: usage: themis run \"task\" --project DIR.".to_string());
        };
        let Some(project) = self.project else {
            return Err("missing --project DIR.".to_string());
        };
        Ok(Command::Run(RunOptions {
            task,
            project,
            provider: self.provider,
            model: self.model,
            base_url: self.base_url,
            api_key: self.api_key,
            yes: self.yes,
            max_turns: self.max_turns,
            skills_file: self.skills_file,
        }))
    }
}

fn run_flag_value(
    args: &[String],
    index: &mut usize,
    flag: &str,
    inline: Option<&str>,
) -> Result<String, String> {
    let value = match inline {
        Some(value) => value.to_owned(),
        None => {
            *index += 1;
            args.get(*index)
                .cloned()
                .ok_or_else(|| format!("flag '{flag}' needs a value."))?
        }
    };
    if value.trim().is_empty() {
        return Err(format!("flag '{flag}' needs a non-empty value."));
    }
    Ok(value)
}

fn parse_run(args: &[String]) -> Result<Command, String> {
    let mut options = PendingRunOptions::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(flag, value)| (flag, Some(value)));
        match flag {
            "-h" | "--help" => return Ok(Command::Help),
            "--project" | "--provider" | "--model" | "--base-url" | "--api-key" | "--max-turns"
            | "--skills-file" => {
                let value = run_flag_value(args, &mut index, flag, inline)?;
                options.set_flag(flag, value)?;
            }
            "--yes" => {
                if inline.is_some() {
                    return Err("flag '--yes' takes no value.".to_string());
                }
                options.yes = true;
            }
            other if other.starts_with('-') => return Err(format!("unknown flag '{other}'.")),
            positional => {
                if options.task.is_some() {
                    return Err(format!("unexpected argument '{positional}'."));
                }
                options.task = Some(positional.to_owned());
            }
        }
        index += 1;
    }
    options.finish()
}

/// Resolves the Go API key: explicit flag, then environment.
fn resolve_api_key(_provider: ProviderKind, explicit: Option<String>) -> Result<String, String> {
    if let Some(key) = explicit.filter(|key| !key.trim().is_empty()) {
        return Ok(key);
    }
    std::env::var("THEMIS_GO_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| {
            "missing OpenCode Go API key: pass --api-key or set THEMIS_GO_API_KEY".to_owned()
        })
}

/// Loads and validates the `--skills-file` JSON array (empty when no flag).
fn load_skills_file(path: Option<&PathBuf>) -> Result<Vec<Skill>, String> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read --skills-file '{}': {e}", path.display()))?;
    let skills: Vec<Skill> = serde_json::from_str(&raw)
        .map_err(|e| format!("invalid --skills-file '{}': {e}", path.display()))?;
    for skill in &skills {
        validate_skill_input(
            &skill.name,
            &skill.instructions,
            &skill.allowed_tools,
            &skill.scripts,
        )
        .map_err(|e| format!("invalid skill '{}' in --skills-file: {e:#}", skill.id))?;
    }
    Ok(skills)
}

/// Executes the `run` command.
async fn run_task_command(options: RunOptions) -> i32 {
    if !options.project.is_dir() {
        eprintln!(
            "error: project dir '{}' does not exist or is not a directory.",
            options.project.display()
        );
        return 2;
    }
    let skills = match load_skills_file(options.skills_file.as_ref()) {
        Ok(skills) => skills,
        Err(error) => {
            eprintln!("error: {error}");
            return 2;
        }
    };
    if let Err(error) = materialize_scripts(&skills, &options.project) {
        eprintln!("error: failed to materialize skill scripts: {error:#}.");
        return 2;
    }
    let api_key = match resolve_api_key(options.provider, options.api_key) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("error: {error}");
            return 2;
        }
    };
    let mut config = ProviderConfig::new(options.provider, api_key);
    if let Some(model) = options.model {
        config = config.with_model(model);
    }
    if let Some(base_url) = options.base_url {
        config = config.with_base_url(base_url);
    }
    let llm = match themis_core::providers::resolve(&config).await {
        Ok(llm) => llm,
        Err(error) => {
            eprintln!(
                "error: failed to resolve provider '{}': {error:#}.",
                options.provider
            );
            return 1;
        }
    };

    let base: Arc<dyn ApprovalHook> = if options.yes {
        Arc::new(AllowAllHook)
    } else {
        Arc::new(StdinHook)
    };
    // Shared between the tools and the runtime so an interactive answer is
    // cached once per tool instead of prompting twice per call.
    let approvals: Arc<dyn ApprovalHook> = CachingApprovals::wrap(base);
    let tools = match boxed_tools(&options.project, Arc::clone(&approvals)) {
        Ok(tools) => tools,
        Err(error) => {
            eprintln!("error: failed to build tools: {error:#}.");
            return 1;
        }
    };

    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(EVENT_CHANNEL_CAPACITY);
    let printer = tokio::spawn(async move {
        while let Some(event) = events_rx.recv().await {
            print_event(&event);
        }
    });
    let outcome = run_task_skilled(
        llm,
        tools,
        options.task,
        approvals,
        options.max_turns,
        events_tx,
        &skills,
    )
    .await;
    let _ = printer.await;
    match outcome {
        Ok(_) => 0,
        Err(_) => 1,
    }
}

/// Prints one run event as a readable line (the failure details ride the event).
fn print_event(event: &RunEvent) {
    match event {
        RunEvent::Started { task, max_turns } => {
            println!("started: {task} (max turns: {max_turns})");
        }
        RunEvent::AssistantText(text) => println!("agent: {text}"),
        RunEvent::ToolCallStarted { tool, summary } => {
            println!("tool start: {tool} - {summary}");
        }
        RunEvent::ToolCallFinished { tool, ok, .. } => println!("tool finish: {tool} ok={ok}"),
        RunEvent::ApprovalDecided { tool, decision } => {
            println!("approval: {tool} -> {decision}");
        }
        RunEvent::ContextCompacting => println!("compacting context"),
        RunEvent::ContextCheckpoint { .. } => println!("context checkpoint saved"),
        RunEvent::Incomplete { result } => println!("agent: {result}"),
        RunEvent::Finished { result } => println!("finished: {result}"),
        RunEvent::Failed { error } => println!("failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(ToString::to_string).collect()
    }

    fn run_options(command: Command) -> RunOptions {
        match command {
            Command::Run(options) => options,
            Command::Help => panic!("expected run command"),
        }
    }

    #[test]
    fn approval_responses_are_case_insensitive_and_deny_by_default() {
        assert!(matches!(
            parse_approval_response("YES\n"),
            Some(Approval::AllowOnce)
        ));
        assert!(matches!(
            parse_approval_response("Always"),
            Some(Approval::AllowAlways)
        ));
        assert!(matches!(
            parse_approval_response("no"),
            Some(Approval::Deny)
        ));
        assert!(matches!(
            parse_approval_response("  "),
            Some(Approval::Deny)
        ));
        assert!(parse_approval_response("later").is_none());
    }

    #[test]
    fn help_flags_show_help() {
        for argv in [["--help"], ["-h"], ["help"]] {
            let words: Vec<&str> = argv.into_iter().collect();
            assert!(matches!(parse_args(&args(&words)), Ok(Command::Help)));
        }
        assert!(matches!(
            parse_args(&args(&["run", "--help"])),
            Ok(Command::Help)
        ));
    }

    #[test]
    fn run_parses_positional_task_and_flags() {
        let command = parse_args(&args(&[
            "run",
            "do the thing",
            "--project",
            "/tmp/proj",
            "--provider",
            "go",
            "--model=gpt-x",
            "--base-url",
            "http://localhost:1",
            "--api-key",
            "secret",
            "--yes",
            "--max-turns",
            "7",
        ]))
        .unwrap();
        let options = run_options(command);
        assert_eq!(options.task, "do the thing");
        assert_eq!(options.project, PathBuf::from("/tmp/proj"));
        assert_eq!(options.provider, ProviderKind::Go);
        assert_eq!(options.model.as_deref(), Some("gpt-x"));
        assert_eq!(options.base_url.as_deref(), Some("http://localhost:1"));
        assert_eq!(options.api_key.as_deref(), Some("secret"));
        assert!(options.yes);
        assert_eq!(options.max_turns, 7);
    }

    #[test]
    fn run_defaults_to_go_with_twenty_turns() {
        let command = parse_args(&args(&["run", "task", "--project", "."])).unwrap();
        let options = run_options(command);
        assert_eq!(options.provider, ProviderKind::Go);
        assert_eq!(options.max_turns, DEFAULT_MAX_TURNS);
        assert!(!options.yes);
        assert!(options.skills_file.is_none());
    }

    #[test]
    fn run_parses_skills_file() {
        let command = parse_args(&args(&[
            "run",
            "task",
            "--project",
            ".",
            "--skills-file",
            "skills.json",
        ]))
        .unwrap();
        let options = run_options(command);
        assert_eq!(options.skills_file, Some(PathBuf::from("skills.json")));

        let command = parse_args(&args(&[
            "run",
            "task",
            "--project",
            ".",
            "--skills-file=other.json",
        ]))
        .unwrap();
        let options = run_options(command);
        assert_eq!(options.skills_file, Some(PathBuf::from("other.json")));
    }

    #[test]
    fn load_skills_file_rejects_missing_and_invalid() {
        assert!(load_skills_file(None).unwrap().is_empty());
        let err = load_skills_file(Some(&PathBuf::from("/nonexistent/skills.json"))).unwrap_err();
        assert!(err.contains("--skills-file"), "{err}");

        let dir = std::env::temp_dir().join(format!("themis-skills-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.json");
        std::fs::write(&bad, "not json").unwrap();
        let err = load_skills_file(Some(&bad)).unwrap_err();
        assert!(err.contains("invalid --skills-file"), "{err}");

        let unknown_tool = dir.join("unknown-tool.json");
        std::fs::write(
            &unknown_tool,
            r#"[{"id":"a","name":"A","description":"d","instructions":"i",
                "allowedTools":["bogus"],"scripts":[]}]"#,
        )
        .unwrap();
        let err = load_skills_file(Some(&unknown_tool)).unwrap_err();
        assert!(err.contains("bogus"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_rejects_bad_input() {
        for argv in [
            vec!["run"],
            vec!["run", "task"],
            vec!["run", "--project", "."],
            vec!["run", "a", "b", "--project", "."],
            vec!["run", "task", "--project", ".", "--nope"],
            vec!["run", "task", "--project", ".", "--provider", "wat"],
            vec!["run", "task", "--project", ".", "--max-turns", "0"],
            vec!["run", "task", "--project", ".", "--max-turns", "lots"],
            vec!["run", "task", "--project"],
            vec!["bogus"],
        ] {
            let words: Vec<&str> = argv;
            assert!(parse_args(&args(&words)).is_err(), "{words:?}");
        }
    }

    #[test]
    fn explicit_key_wins() {
        assert_eq!(
            resolve_api_key(ProviderKind::Go, Some("flag-key".to_string())).unwrap(),
            "flag-key"
        );
    }
}
