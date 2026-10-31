//! A shell held to named commands: a plugin's `Bash(git diff:*)` means those and nothing else.
//!
//! It reads commands with the parser deny rules use, but the question is turned around: a deny
//! rule asks whether a forbidden command might run, this asks whether every command that would
//! run is named, so whatever can't be read is refused. Only `sh -c` and its kin are seen through;
//! another wrapper (`env`, `sudo`, `xargs`, `timeout`) must be named itself, since it can change
//! what a named command means (`env PATH=. git diff`). For the same reason a script's
//! assignments and redirections are refused, and programs are compared as written, so
//! `./git diff` is not `git diff`.
//!
//! Patterns are matched word by word, so a `*` can't reach past one word to find the name it wants
//! further along (`python3 evil.py check.py` against `python3 *check.py*`). Programs that run code
//! they are given ([`Runner`]) are held harder: every option must be named by the pattern as a
//! whole word, however it is bundled, and what they run must be spelled out, so `python3:*`,
//! `git:*` and `npx *` allow nothing past the bare program. That is an allow-list, unlike the
//! options in [`escape_options`], which only cover a few well-known programs.
//!
//! **What this is not.** It narrows what a call may start; it is not a sandbox. A named program
//! does what that program does: `python3 check.py` runs whatever `check.py` holds, `npm test` runs
//! whatever `package.json` says, and a program nobody listed here may have its own way to run
//! another (`rg --pre`, `gh extension`). The sandbox and the approval rules still apply to all of
//! it, and they are what contains it.

use super::{program_basename, wrapped};

/// Wrapper levels followed before giving up.
const MAX_DEPTH: usize = 4;
/// Longest pattern kept, and the most a profile may name.
pub const MAX_PATTERN_CHARS: usize = 200;
pub const MAX_PATTERNS: usize = 64;

/// Redirections that write nothing a person would want guarded.
const HARMLESS_REDIRECTIONS: [&str; 3] = ["2>&1", "2>/dev/null", ">/dev/null"];

/// A program that runs code it is given.
#[derive(Clone, Copy, PartialEq)]
enum Runner {
    /// The first word after its options is a script or a program: a pattern may leave part of a
    /// name open (`*check.py*`) but not all of it.
    Script,
    /// The first word after its options chooses what runs (`git status`, `npx tsc`, `make test`):
    /// the pattern must spell it out.
    Subcommand,
}

fn runner(program: &str) -> Option<Runner> {
    match program {
        name if name.starts_with("python") || name.starts_with("pypy") => Some(Runner::Script),
        "node" | "nodejs" | "ruby" | "perl" | "php" | "deno" | "bun" | "lua" | "tsx"
        | "ts-node" | "rscript" | "osascript" | "sh" | "bash" | "zsh" | "dash" | "ksh" | "mksh"
        | "ash" | "fish" | "pwsh" | "powershell" | "awk" | "gawk" | "mawk" | "nawk" | "sed"
        | "gsed" => Some(Runner::Script),
        "git" | "npx" | "pnpx" | "bunx" | "uvx" | "uv" | "pipx" | "npm" | "pnpm" | "yarn"
        | "make" | "gmake" | "cargo" | "go" | "docker" | "podman" | "ssh" | "gh" | "kubectl" => {
            Some(Runner::Subcommand)
        }
        _ => None,
    }
}

/// Options of a few programs that make it run something else, wherever they appear. Not a complete
/// list and it can't be: see the note at the top.
fn escape_options(program: &str) -> &'static [&'static str] {
    match program {
        "find" => &[
            "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprint", "-fprintf", "-fls",
            "-fprint0",
        ],
        "git" => &[
            "--ext-diff",
            "--textconv",
            "--upload-pack",
            "--receive-pack",
            "--exec",
            "--open-files-in-pager",
        ],
        "tar" => &[
            "--to-command",
            "--use-compress-program",
            "--checkpoint-action",
            "-I",
        ],
        "rsync" => &["-e", "--rsh"],
        "rg" => &["--pre"],
        "fd" | "fdfind" => &["-x", "-X", "--exec", "--exec-batch"],
        _ => &[],
    }
}

/// A pattern as Claude writes them, in words: `git diff:*` and `git diff *` are `git diff` with or
/// without more words; a last word ending in `*` takes any further words too.
struct Pattern<'a> {
    words: Vec<&'a str>,
    /// Any number of further words.
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

    /// Whether `word` is one of its own words: a flag exempted must be spelled out whole.
    fn names(&self, word: &str) -> bool {
        self.words.contains(&word)
    }

    fn covers(&self, command: &[String]) -> bool {
        let last = self.words.len().saturating_sub(1);
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
        !self.words.is_empty()
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

/// Whether a runner may run this under this pattern. Everything before the first word that isn't
/// an option must be named whole, so `-Ic`, `-pe` and `--eval=x` fail for not being named, with
/// no list of letters to get wrong. That first word must then be spelled out in the pattern.
fn runner_allowed(kind: Runner, command: &[String], pattern: &Pattern<'_>) -> bool {
    for (index, word) in command.iter().enumerate().skip(1) {
        if word.starts_with('-') || word.starts_with('+') {
            if !pattern.names(word) {
                return false;
            }
            continue;
        }
        let Some(wanted) = pattern.words.get(index) else {
            return false;
        };
        return match kind {
            Runner::Subcommand => !wanted.contains('*'),
            Runner::Script => wanted.chars().any(|c| !matches!(c, '*' | '.' | '/')),
        };
    }
    // Nothing to run but what it was given as options, all of them named.
    true
}

/// A long option, whole or abbreviated (git accepts `--ext-d`), or a short one by its letters.
fn is_escape(word: &str, escapes: &[&str]) -> bool {
    let flag = word.split('=').next().unwrap_or(word);
    escapes.iter().any(|escape| {
        if let Some(long) = escape.strip_prefix("--") {
            flag.strip_prefix("--")
                .is_some_and(|name| name.len() >= 3 && long.starts_with(name))
        } else if escape.len() == 2 {
            // `-e` also found bundled, as in `-ve`.
            flag.starts_with('-') && !flag.starts_with("--") && flag.contains(&escape[1..])
        } else {
            flag == *escape
        }
    })
}

/// One simple command, not looking inside it.
fn named(command: &[String], patterns: &[String]) -> bool {
    let Some(program) = command.first() else {
        return false;
    };
    let name = program_basename(program);
    let escapes = escape_options(&name);
    let kind = runner(&name);
    patterns.iter().any(|text| {
        let pattern = Pattern::parse(text);
        pattern.covers(command)
            && kind.is_none_or(|kind| runner_allowed(kind, command, &pattern))
            && !command[1..]
                .iter()
                .any(|word| is_escape(word, escapes) && !pattern.names(word))
    })
}

fn in_scope(command: &[String], patterns: &[String], depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    if let Some(script) = wrapped::shell_script_text(command) {
        let Some(commands) = wrapped::split_script_keeping_assignments(script) else {
            return false;
        };
        return commands.iter().all(|words| {
            !words
                .first()
                .is_some_and(|word| wrapped::is_assignment(word))
                && !words.iter().any(|word| {
                    word.contains('>') && !HARMLESS_REDIRECTIONS.contains(&word.as_str())
                })
                && in_scope(&without_harmless(words), patterns, depth + 1)
        });
    }
    if !named(command, patterns) {
        return false;
    }
    // A named wrapper still can't run what isn't named.
    wrapped::wrapped_commands(command).is_some_and(|inner| {
        inner
            .iter()
            .all(|command| in_scope(command, patterns, depth + 1))
    })
}

fn without_harmless(words: &[String]) -> Vec<String> {
    words
        .iter()
        .filter(|word| !HARMLESS_REDIRECTIONS.contains(&word.as_str()))
        .cloned()
        .collect()
}

/// Whether every command `argv` would run is one `patterns` names.
#[must_use]
pub fn command_in_scope(argv: &[String], patterns: &[String]) -> bool {
    !argv.is_empty() && in_scope(argv, patterns, 0)
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
        assert!(allowed(&["ls"], &["ls"]));
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
        assert!(allowed(
            &["sh", "-c", "git diff && git diff --stat"],
            GIT_DIFF
        ));
        assert!(allowed(&["sh", "-c", "git diff 2>&1"], GIT_DIFF));
    }

    #[test]
    fn anything_else_is_refused() {
        for line in [
            &["git", "push"][..],
            &["git", "diffx"],
            &["git"],
            &["rm", "-rf", "x"],
            // Another spelling, or a path, is another program.
            &["./git", "diff"],
            &["/tmp/git", "diff"],
            &["git", "-C", ".", "push"],
            &["git", "-C", ".", "diff"],
            // Chains and wrappers hide nothing.
            &["sh", "-c", "git diff && rm x"],
            &["sh", "-c", "git diff; rm x"],
            &["sh", "-c", "git diff | tee x"],
            &["sh", "-c", "git diff $(rm x)"],
            &["sh", "-c", "git diff `rm x`"],
            &["bash", "-lc", "sh -c 'rm x'"],
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
            // Options that run something else.
            &["git", "diff", "--ext-diff"],
            &["git", "diff", "--ext-d"],
            &["git", "--config-env=a=b", "diff"],
            &["git", "-c", "core.pager=x", "diff"],
            &[],
        ] {
            assert!(!allowed(line, GIT_DIFF), "{line:?}");
        }
        assert!(!allowed(
            &["find", ".", "-name", "x", "-exec", "rm", "{}", ";"],
            &["find *"]
        ));
        assert!(!allowed(&["find", ".", "-delete"], &["find *"]));
        assert!(!allowed(
            &["python3", "-c", "evil", "extract_aspects.py"],
            &["python3 *extract_aspects.py*"]
        ));
        assert!(!allowed(&["node", "--eval=x", "a.js"], &["node *"]));
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
        // What is spelled out runs, options included.
        for (line, pattern) in [
            (&["python3", "-m", "pytest", "x"][..], "python3 -m pytest *"),
            (&["python3", "check.py", "-v"], "python3 check.py:*"),
            (&["node", "run.js"], "node run.js:*"),
            (&["bash", "x/verify.sh"], "bash *verify*"),
            (&["git", "status"], "git status:*"),
            (&["npx", "tsc", "--noEmit"], "npx tsc:*"),
            (&["make", "test"], "make test"),
            (&["gh", "pr", "view", "1"], "gh pr view *"),
            (&["node", "--version"], "node --version"),
        ] {
            assert!(allowed(line, &[pattern]), "{line:?} under {pattern}");
        }
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
        assert!(allowed(&["find", ".", "-exec", "x"], &["find . -exec x"]));
    }

    #[test]
    fn a_star_stays_within_its_word() {
        assert!(!allowed(&["gh", "pr", "evil", "view"], &["gh pr * view x"]));
        assert!(allowed(&["ls", "a", "b"], &["ls *"]));
        assert!(!allowed(&["cat", "a", "b"], &["cat a"]));
        assert!(allowed(&["cat", "a", "b"], &["cat * b"]));
        assert!(!allowed(&["cat", "a", "x", "b"], &["cat * b"]));
    }

    #[test]
    fn a_wrapper_or_option_counts_only_when_it_is_named() {
        assert!(allowed(
            &["timeout", "5", "git", "diff"],
            &["timeout *", "git diff:*"]
        ));
        // Named, but what it runs isn't.
        assert!(!allowed(
            &["timeout", "5", "rm", "x"],
            &["timeout *", "git diff:*"]
        ));
        assert!(!allowed(&["env", "sh", "-c", "rm x"], &["env *"]));
        assert!(allowed(&["python3", "-c", "1"], &["python3 -c 1"]));
        assert!(allowed(&["find", ".", "-exec", "x"], &["find * -exec *"]));
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
