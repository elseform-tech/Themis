//! A shell held to named commands: a plugin's `Bash(git diff:*)` means those and nothing else.
//!
//! It passes only what it can positively confirm; any other form is refused, harmless or not, and
//! the subagent reports the refusal. Three rounds of review found ways past a list of bad options
//! per program, because such a list can't be complete, so there are none here.
//!
//! A command passes when all of this holds:
//!
//! 1. It is one simple command: direct, or the only command of an `sh -c` script. No chains,
//!    pipes, substitutions, assignments or redirections to files, and no wrapper (`env`, `sudo`,
//!    `xargs`, `timeout`), even a named one, since those change what a named command means.
//! 2. Its words fit a pattern, word by word, with its program as written (`./git` isn't `git`,
//!    and a pattern can't leave the program open). A `*` stays within its word, except that
//!    `:*` and a final ` *` or `*` take any further words.
//! 3. If the program can run others ([`Class`]), every option and `NAME=value` word is named by
//!    the pattern or (options only) on a small allow-list for that subcommand, and what it runs
//!    is spelled out.
//!
//! **What this does not do.** It narrows what a subagent can start; it is not a security boundary.
//! A program that runs project code (`npm test`, `pytest`, `make test`, `python3 check.py`) runs
//! whatever that code does, and a program in none of the classes below does what it does with
//! whatever words the pattern lets through. The OS sandbox is the wall.

use super::{program_basename, wrapped};

/// Longest pattern kept, and the most a profile may name.
pub const MAX_PATTERN_CHARS: usize = 200;
pub const MAX_PATTERNS: usize = 64;

/// Redirections that write nothing.
const HARMLESS_REDIRECTIONS: [&str; 3] = ["2>&1", "2>/dev/null", ">/dev/null"];

/// Programs whose whole purpose is to run another; none passes, whatever a pattern names.
const WRAPPERS: [&str; 21] = [
    "env",
    "sudo",
    "doas",
    "nice",
    "ionice",
    "timeout",
    "gtimeout",
    "xargs",
    "parallel",
    "stdbuf",
    "caffeinate",
    "nohup",
    "setsid",
    "time",
    "command",
    "builtin",
    "exec",
    "eval",
    "busybox",
    "script",
    "watch",
];

/// How a program that can run others is held.
#[derive(Clone, Copy, PartialEq)]
enum Class {
    /// `python3 check.py`, `sh x.sh`, `awk 'p' f`: options before the first plain word are named;
    /// a pattern may leave part of that word open (`*check.py*`), not all of it. What follows
    /// belongs to the script.
    Script,
    /// `npx tsc`: as above, with the name it fetches and runs spelled out whole.
    Launcher,
    /// `git diff`, `make test`, `tar tf`: the first plain word is spelled out whole, and every
    /// option anywhere is named or on the allow-list for it.
    Subcommand,
    /// `find`, `rg`: plain words are free; every option is named or on the allow-list.
    Options,
}

fn class_of(program: &str) -> Option<Class> {
    match program {
        name if name.starts_with("python") || name.starts_with("pypy") => Some(Class::Script),
        "node" | "nodejs" | "ruby" | "perl" | "php" | "deno" | "bun" | "lua" | "tsx"
        | "ts-node" | "rscript" | "osascript" | "sh" | "bash" | "zsh" | "dash" | "ksh" | "mksh"
        | "ash" | "fish" | "pwsh" | "powershell" | "cmd" | "awk" | "gawk" | "mawk" | "nawk"
        | "sed" | "gsed" => Some(Class::Script),
        "npx" | "pnpx" | "bunx" | "uvx" => Some(Class::Launcher),
        "git" | "npm" | "pnpm" | "yarn" | "make" | "gmake" | "cargo" | "go" | "docker"
        | "podman" | "ssh" | "scp" | "gh" | "kubectl" | "tar" | "gtar" | "rsync" | "pip"
        | "pip3" | "pipx" | "uv" | "poetry" | "composer" | "bundle" | "gem" | "rake" | "mvn"
        | "gradle" => Some(Class::Subcommand),
        "find" | "gfind" | "fd" | "fdfind" | "rg" | "ag" | "sort" | "vim" | "vi" | "nvim"
        | "less" | "more" | "man" | "curl" | "wget" | "pytest" | "py.test" | "tox" | "jest"
        | "vitest" | "mocha" | "tsc" | "eslint" | "ruff" | "mypy" => Some(Class::Options),
        _ => None,
    }
}

/// Options that only choose what is shown, by program and subcommand. Short on purpose: anything
/// else has to be named by the pattern.
fn allowed_options(program: &str, subcommand: Option<&str>) -> &'static [&'static str] {
    const GIT_SHOWING: &[&str] = &[
        "--stat",
        "--shortstat",
        "--numstat",
        "--name-only",
        "--name-status",
        "--no-color",
        "--no-ext-diff",
        "--oneline",
        "--patch",
        "-p",
        "--unified",
        "--format",
        "--pretty",
        "--abbrev-commit",
        "--no-pager",
    ];
    match (program, subcommand) {
        ("git", Some("diff")) => &[
            "--stat",
            "--shortstat",
            "--numstat",
            "--name-only",
            "--name-status",
            "--no-color",
            "--no-ext-diff",
            "--patch",
            "-p",
            "--unified",
            "--cached",
            "--staged",
        ],
        ("git", Some("show")) => GIT_SHOWING,
        ("git", Some("log")) => &[
            "--stat",
            "--shortstat",
            "--numstat",
            "--name-only",
            "--name-status",
            "--no-color",
            "--no-ext-diff",
            "--oneline",
            "--patch",
            "-p",
            "--format",
            "--pretty",
            "--abbrev-commit",
            "--graph",
            "--decorate",
            "--all",
            "--reverse",
            "--no-merges",
            "--since",
            "--until",
            "--author",
            "--grep",
            "-n",
        ],
        ("git", Some("status")) => &["--short", "-s", "--porcelain", "--branch", "-b"],
        ("git", Some("branch")) => &["--show-current", "--list", "-a", "-r", "--all"],
        ("git", Some("rev-parse")) => &["--short", "--abbrev-ref", "--show-toplevel", "HEAD"],
        ("git", Some("symbolic-ref")) => &["--short", "-q", "--quiet"],
        ("gh", Some(_)) => &[
            "--json",
            "-q",
            "--jq",
            "--paginate",
            "-R",
            "--repo",
            "-L",
            "--limit",
            "--state",
        ],
        ("rg", _) => &[
            "-n",
            "-i",
            "-l",
            "-c",
            "-w",
            "-F",
            "--line-number",
            "--files",
        ],
        _ => &[],
    }
}

/// A word that is an option. A lone `-` means standard input, which holds a program as well as an
/// option could.
fn is_option(word: &str) -> bool {
    word.starts_with('-') || word.starts_with('+')
}

/// A word that sets something for these programs as an option does: `SHELL=/tmp/x` to `make`.
fn is_setting(word: &str) -> bool {
    is_option(word) || wrapped::is_assignment(word)
}

/// An option without its value: `--eval` from `--eval=x`.
fn flag_of(word: &str) -> &str {
    word.split('=').next().unwrap_or(word)
}

/// A pattern as Claude writes them, in words.
struct Pattern<'a> {
    words: Vec<&'a str>,
    /// Any number of further words: `:*`, or a final ` *`.
    rest: bool,
}

impl<'a> Pattern<'a> {
    fn parse(text: &'a str) -> Self {
        let (text, open) = match text.strip_suffix(":*") {
            Some(text) => (text, true),
            None => (text, false),
        };
        let mut words: Vec<&str> = text.split_whitespace().collect();
        let mut rest = open;
        if words.len() > 1 && words.last() == Some(&"*") {
            words.pop();
            rest = true;
        }
        Self { words, rest }
    }

    /// Whether it has this option as a word of its own, value or no value. A flag's letters
    /// inside another word don't count.
    fn names(&self, option: &str) -> bool {
        self.words
            .iter()
            .any(|word| *word == option || flag_of(word) == flag_of(option))
    }

    fn covers(&self, command: &[String]) -> bool {
        let Some(program) = self.words.first() else {
            return false;
        };
        if program.contains('*') {
            return false;
        }
        let last = self.words.len() - 1;
        for (index, wanted) in self.words.iter().enumerate() {
            let Some(word) = command.get(index) else {
                return false;
            };
            if !glob_matches(wanted, word) {
                return false;
            }
            if index == last && command.len() > self.words.len() {
                return self.rest || (wanted.ends_with('*') && index > 0);
            }
        }
        true
    }
}

/// Whether `text` fits `pattern`, where `*` stands for any text.
fn glob_matches(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let mut parts = parts.peekable();
    if parts.peek().is_none() {
        return rest.is_empty();
    }
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return rest.ends_with(part);
        }
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

/// Whether a program in `class` may run these words under this pattern.
fn class_allows(class: Class, program: &str, command: &[String], pattern: &Pattern<'_>) -> bool {
    // The first plain word: a script, a package or a subcommand.
    let first_plain = command
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, word)| !is_setting(word));
    let subcommand = first_plain.map(|(_, word)| word.as_str());
    let allowed = allowed_options(program, subcommand);
    let confirmed = |word: &str| {
        pattern.words.contains(&word)
            || (is_option(word) && pattern.names(word))
            || (is_option(word) && allowed.contains(&flag_of(word)))
            // `git log -5`.
            || (program == "git"
                && subcommand == Some("log")
                && word.len() > 1
                && word[1..].bytes().all(|byte| byte.is_ascii_digit()))
    };
    let spelled_out = |index: usize, whole: bool| {
        pattern.words.get(index).is_some_and(|wanted| {
            if whole {
                !wanted.contains('*')
            } else {
                wanted.chars().any(|c| !matches!(c, '*' | '.' | '/'))
            }
        })
    };
    // Everything before the first plain word is the program's own, in every class.
    let before = first_plain.map_or(command.len(), |(index, _)| index);
    if !command[1..before].iter().all(|word| confirmed(word)) {
        return false;
    }
    // Only git is known to take what follows `--` as plain paths: `make test -- SHELL=/x` still
    // reads the assignment, so elsewhere `--` is an option like any other and ends nothing.
    let every_option = || {
        let rest = &command[before..];
        let checked = if program == "git" {
            let end = rest.iter().position(|word| word == "--");
            &rest[..end.unwrap_or(rest.len())]
        } else {
            rest
        };
        checked
            .iter()
            .filter(|word| is_setting(word))
            .all(|word| confirmed(word))
    };
    match (class, first_plain) {
        (Class::Options, _) => every_option(),
        (_, None) => true,
        (Class::Script, Some((index, _))) => spelled_out(index, false),
        (Class::Launcher, Some((index, _))) => spelled_out(index, true),
        (Class::Subcommand, Some((index, _))) => spelled_out(index, true) && every_option(),
    }
}

/// The one simple command this runs, if it is one: itself, or the only command of a shell script.
fn simple_command(command: &[String]) -> Option<Vec<String>> {
    let Some(script) = wrapped::shell_script_text(command) else {
        return Some(command.to_vec());
    };
    let commands = wrapped::split_script_keeping_assignments(script)?;
    let [words] = commands.as_slice() else {
        return None;
    };
    if words
        .first()
        .is_some_and(|word| wrapped::is_assignment(word))
    {
        return None;
    }
    let words: Vec<String> = words
        .iter()
        .filter(|word| !HARMLESS_REDIRECTIONS.contains(&word.as_str()))
        .cloned()
        .collect();
    let plain = !words
        .iter()
        .any(|word| word.contains('>') || word.contains('<'));
    // A script's only command being another script is more wrapping than anyone needs.
    (plain && wrapped::shell_script_text(&words).is_none()).then_some(words)
}

/// Whether what `argv` runs is one simple command `patterns` names; see the top of this file.
#[must_use]
pub fn command_in_scope(argv: &[String], patterns: &[String]) -> bool {
    let Some(command) = simple_command(argv) else {
        return false;
    };
    let Some(program) = command.first() else {
        return false;
    };
    let name = program_basename(program);
    // The parser's own wrappers count too, should this list miss one.
    if WRAPPERS.contains(&name.as_str())
        || !wrapped::wrapped_commands(&command).is_some_and(|inner| inner.is_empty())
    {
        return false;
    }
    let class = class_of(&name);
    patterns.iter().any(|text| {
        let pattern = Pattern::parse(text);
        pattern.covers(&command)
            && class.is_none_or(|class| class_allows(class, &name, &command, &pattern))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn allowed(line: &[&str], patterns: &[&str]) -> bool {
        let argv: Vec<String> = line.iter().map(|word| (*word).to_owned()).collect();
        let patterns: Vec<String> = patterns.iter().map(|word| (*word).to_owned()).collect();
        command_in_scope(&argv, &patterns)
    }

    const GIT_DIFF: &[&str] = &["git diff:*"];

    #[test]
    fn the_forms_plugins_write_match_what_they_mean() {
        assert!(allowed(&["git", "diff"], GIT_DIFF));
        assert!(allowed(&["git", "diff", "--stat", "a b"], GIT_DIFF));
        assert!(allowed(&["gh", "pr", "view", "1"], &["gh pr view *"]));
        assert!(allowed(&["gh", "pr", "view"], &["gh pr view *"]));
        assert!(allowed(
            &["gh", "pr", "view", "--json", "x"],
            &["gh pr view *"]
        ));
        assert!(allowed(&["ls"], &["ls"]));
        assert!(allowed(&["ls", "-la", "x"], &["ls *"]));
        assert!(allowed(
            &["gh", "api", "repos/a/b/pulls/1/comments", "-q", "x"],
            &["gh api repos/*/pulls/*/comments *"]
        ));
        assert!(allowed(
            &["python3", "tools/extract_aspects.py", "x"],
            &["python3 *extract_aspects.py*"]
        ));
        assert!(allowed(
            &["find", ".", "-name", "x", "-type", "f"],
            &["find * -name * -type *"]
        ));
        assert!(allowed(&["sh", "-c", "git diff --stat"], GIT_DIFF));
        assert!(allowed(&["sh", "-c", "git diff 2>&1"], GIT_DIFF));
        assert!(allowed(&["git", "log", "-5", "--oneline"], &["git log:*"]));
    }

    /// Everything three reviews named, and what followed from them. The forms are refused for not
    /// being confirmed, so a form nobody thought of is refused the same way.
    #[test]
    fn anything_it_cannot_confirm_is_refused() {
        for line in [
            &["git", "push"][..],
            &["git", "diffx"],
            &["git"],
            &["rm", "-rf", "x"],
            // Another spelling, or a path, is another program.
            &["./git", "diff"],
            &["/tmp/git", "diff"],
            &["/usr/bin/git", "diff"],
            &["git", "-C", ".", "push"],
            &["git", "-C", ".", "diff"],
            // Not one simple command.
            &["sh", "-c", "git diff && git diff"],
            &["sh", "-c", "git diff && rm x"],
            &["sh", "-c", "git diff; rm x"],
            &["sh", "-c", "git diff | tee x"],
            &["sh", "-c", "git diff | cat"],
            &["sh", "-c", "git diff $(rm x)"],
            &["sh", "-c", "git diff `rm x`"],
            &["bash", "-lc", "sh -c 'git diff'"],
            &["sh", "-c", "$X diff"],
            &["sh", "-c", "git diff \"$(rm x)\""],
            &["sh", "-c", "git diff\nrm x"],
            &["sh", "x.sh"],
            &["eval", "git diff"],
            &["env", "git", "diff"],
            &["env", "PATH=.", "git", "diff"],
            &["sudo", "git", "diff"],
            &["xargs", "git", "diff"],
            &["timeout", "5", "git", "diff"],
            &["busybox", "sh", "-c", "git diff"],
            // Assignments and redirections change what it does.
            &["sh", "-c", "PATH=. git diff"],
            &["sh", "-c", "GIT_EXTERNAL_DIFF=x git diff"],
            &["sh", "-c", "git diff > x"],
            &["sh", "-c", "git diff>x"],
            &["sh", "-c", "git diff >> x"],
            &["sh", "-c", "git diff < x"],
            // Any option git isn't known to only show with, however it is spelled.
            &["git", "diff", "--ext-diff"],
            &["git", "diff", "--ext-d"],
            &["git", "diff", "--output=x"],
            &["git", "diff", "--no-index", "a", "b"],
            &["git", "diff", "-c", "core.pager=x"],
            &["git", "-c", "core.pager=x", "diff"],
            &["git", "--config", "a=b", "diff"],
            &["git", "--config-env=a=b", "diff"],
            &["git", "--exec-path=.", "diff"],
            &["git", "-u", "diff"],
            &["git", "diff", "-u"],
            &[],
        ] {
            assert!(!allowed(line, GIT_DIFF), "{line:?}");
        }
        // A wrapper stays refused even when a pattern names it.
        for (line, pattern) in [
            (&["timeout", "5", "git", "diff"][..], "timeout *"),
            (&["env", "git", "diff"], "env:*"),
            (&["xargs", "git", "diff"], "xargs *"),
            (&["sudo", "ls"], "sudo:*"),
            (&["/usr/bin/env", "python3", "-c", "x"], "/usr/bin/env *"),
        ] {
            assert!(!allowed(line, &[pattern]), "{line:?} under {pattern}");
        }
        // An option that isn't confirmed, wherever it sits.
        for (line, pattern) in [
            (
                &["find", ".", "-name", "x", "-exec", "rm", "{}", ";"][..],
                "find *",
            ),
            (&["find", ".", "-delete"], "find *"),
            (&["find", ".", "-name", "x", "-newevil"], "find . -name *"),
            (&["tar", "xI", "evil", "-f", "x"], "tar *"),
            (&["tar", "xI", "evil"], "tar:*"),
            (&["tar", "-xI", "evil", "-f", "x"], "tar *"),
            (&["tar", "tf", "x", "--to-command=evil"], "tar tf *"),
            (&["tar", "tf", "x", "-I", "evil"], "tar tf *"),
            (&["rsync", "-e", "evil", "a", "b"], "rsync *"),
            (&["rg", "--pre", "evil", "x"], "rg *"),
            (&["rg", "--pre=evil", "x"], "rg *"),
            (&["fd", "-x", "evil"], "fd *"),
            (&["make", "test", "-f", "evil"], "make test:*"),
            (&["npm", "test", "--prefix", "x"], "npm test:*"),
            (&["node", "--eval=x", "a.js"], "node *"),
        ] {
            assert!(!allowed(line, &[pattern]), "{line:?} under {pattern}");
        }
    }

    #[test]
    fn a_runner_needs_every_option_named_whole_and_what_it_runs_spelled_out() {
        let tool = &["python3 *tool.py*"];
        assert!(allowed(&["python3", "x/tool.py", "-c", "--eval"], tool));
        for line in [
            &["python3", "-c", "x"][..],
            &["python3", "-Ic", "x", "tool.py"],
            &["python3", "-c", "x", "tool.py"],
            &["python3", "-", "tool.py"],
            &["python3", "-m", "x", "tool.py"],
            &["python3", "evil.py", "tool.py"],
            &["python3.12", "-Ic", "x", "tool.py"],
            &["/usr/bin/python3", "x/tool.py"],
        ] {
            assert!(!allowed(line, tool), "{line:?}");
        }
        // Catch-alls allow nothing past the bare program.
        for (line, pattern) in [
            (&["python3", "x.py"][..], "python3:*"),
            (&["python3", "x.py"], "python3 *"),
            (&["node", "x.js"], "node:*"),
            (&["node", "-pe", "x"], "node:*"),
            (&["bash", "x.sh"], "bash:*"),
            (&["ruby", "-e", "x"], "ruby *"),
            (&["awk", "x"], "awk *"),
            (&["sed", "-n", "e x"], "sed -n *"),
            (&["git", "push"], "git:*"),
            (&["git", "push"], "git *"),
            (&["git", "-c", "a=b", "status"], "git *"),
            (&["git", "--exec-path=.", "status"], "git status:*"),
            (&["npx", "cowsay"], "npx:*"),
            (&["npx", "-p", "evil", "tsc"], "npx tsc:*"),
            (&["npm", "run", "x"], "npm *"),
            (&["make", "-f", "x"], "make:*"),
            (&["make", "x"], "make:*"),
            (&["cargo", "run"], "cargo *"),
            (&["gh", "api", "x"], "gh:*"),
        ] {
            assert!(!allowed(line, &[pattern]), "{line:?} under {pattern}");
        }
        // A program can't be left open.
        assert!(!allowed(&["git", "status"], &["*git status"]));
        assert!(!allowed(&["rm", "x"], &["r* x"]));
        // What is spelled out runs, options included.
        for (line, pattern) in [
            (&["python3", "-m", "pytest", "x"][..], "python3 -m pytest *"),
            (&["python3", "check.py", "-v"], "python3 check.py:*"),
            (&["node", "run.js"], "node run.js:*"),
            (&["bash", "x/verify.sh"], "bash *verify*"),
            (&["git", "status"], "git status:*"),
            (&["npx", "tsc", "--noEmit"], "npx tsc:*"),
            (&["make", "test"], "make test"),
            (&["node", "--version"], "node --version"),
            (&["tar", "tf", "x.tar"], "tar tf *"),
            (&["find", ".", "-exec", "x"], "find . -exec x"),
            (&["git", "log", "--output=x"], "git log --output=*"),
        ] {
            assert!(allowed(line, &[pattern]), "{line:?} under {pattern}");
        }
    }

    /// A `NAME=value` word sets something for these programs, as an option does.
    #[test]
    fn a_setting_word_is_confirmed_like_an_option() {
        for (line, pattern) in [
            (&["make", "test", "SHELL=/tmp/x"][..], "make test:*"),
            (&["make", "test", "CC=/tmp/x"], "make test *"),
            (&["make", "SHELL=/tmp/x", "test"], "make test:*"),
            (&["make", "test", "--", "SHELL=/tmp/x"], "make test:*"),
            (&["gmake", "test", "--", "CC=/tmp/x"], "gmake test:*"),
            (
                &["npm", "test", "--", "--script-shell=/tmp/x"],
                "npm test:*",
            ),
            (&["cargo", "+evil", "test"], "cargo test:*"),
            (&["cargo", "test", "--config=x"], "cargo test:*"),
            (&["pytest", "-p", "evil"], "pytest:*"),
            (&["pytest", "-c", "evil.ini"], "pytest *"),
            (&["pytest", "--rootdir=x"], "pytest:*"),
            (&["npm", "run", "x", "--script-shell=/tmp/x"], "npm run x:*"),
            (&["jest", "--config=evil.js"], "jest:*"),
        ] {
            assert!(!allowed(line, &[pattern]), "{line:?} under {pattern}");
        }
        // Spelled out, it is what the pattern says; so is a plain target or path.
        assert!(allowed(&["make", "test", "CC=gcc"], &["make test CC=gcc"]));
        assert!(allowed(&["make", "test", "unit"], &["make test:*"]));
        assert!(allowed(&["git", "diff", "--", "a=b.txt"], GIT_DIFF));
        assert!(allowed(&["pytest", "tests/a.py"], &["pytest:*"]));
        // What follows a script belongs to the script.
        assert!(allowed(
            &["python3", "check.py", "A=1"],
            &["python3 check.py:*"]
        ));
    }

    #[test]
    fn a_flag_counts_only_when_the_pattern_has_it_as_a_whole_word() {
        // Its letters inside another word exempt nothing.
        assert!(!allowed(&["python3", "-c", "x"], &["python3 -cfoo *"]));
        assert!(!allowed(&["python3", "-c", "x"], &["python3 check-c.py"]));
        assert!(!allowed(
            &["find", ".", "-exec", "x"],
            &["find * *-exec-thing* *"]
        ));
        assert!(!allowed(
            &["git", "diff", "--ext-diff"],
            &["git diff *--ext-diff-x*"]
        ));
    }

    #[test]
    fn a_star_stays_within_its_word() {
        assert!(!allowed(&["gh", "pr", "evil", "view"], &["gh pr * view x"]));
        assert!(allowed(&["ls", "a", "b"], &["ls *"]));
        assert!(!allowed(&["cat", "a", "b"], &["cat a"]));
        assert!(allowed(&["cat", "a", "b"], &["cat * b"]));
        assert!(!allowed(&["cat", "a", "x", "b"], &["cat * b"]));
    }

    proptest! {
        /// Whatever is wrapped how: a command with `rm` as a program is never in scope, and
        /// nothing panics.
        #[test]
        fn a_command_it_does_not_name_never_gets_through(
            before in "[a-z ;&|]{0,12}",
            after in "[a-z ;&|$()`'\"]{0,12}",
            wrapper in prop_oneof![
                Just(vec!["sh", "-c"]),
                Just(vec!["bash", "-lc"]),
                Just(vec!["dash", "-c"]),
            ],
        ) {
            let script = format!("{before}; rm x {after}");
            let mut argv: Vec<String> = wrapper.iter().map(|word| (*word).to_owned()).collect();
            argv.push(script);
            prop_assert!(!command_in_scope(&argv, &["git diff:*".to_owned()]));
        }

        #[test]
        fn any_text_is_read_without_panicking(
            argv in proptest::collection::vec("\\PC{0,12}", 0..5),
            pattern in "\\PC{0,12}",
        ) {
            let _ = command_in_scope(&argv, &[pattern]);
        }

        /// A plain command's program must be exactly what a word-prefix pattern names.
        #[test]
        fn a_prefix_pattern_never_matches_another_program(program in "[a-z]{1,6}") {
            prop_assume!(program != "git");
            let argv = vec![program, "diff".to_owned()];
            prop_assert!(!command_in_scope(&argv, &["git diff:*".to_owned()]));
        }
    }
}
