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
//! Past that a named program does what that program does: `python3 x.py` runs `x.py`. The
//! sandbox and the approval rules still apply to it.

use super::{program_basename, wrapped};

/// Wrapper levels followed before giving up.
const MAX_DEPTH: usize = 4;
/// Longest pattern kept, and the most a profile may name.
pub const MAX_PATTERN_CHARS: usize = 200;
pub const MAX_PATTERNS: usize = 64;

/// Redirections that write nothing a person would want guarded.
const HARMLESS_REDIRECTIONS: [&str; 3] = ["2>&1", "2>/dev/null", ">/dev/null"];

/// Options that make a program run something else; a pattern that spells one out allows it.
fn escape_options(program: &str) -> &'static [&'static str] {
    match program {
        "find" => &[
            "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprint", "-fprintf", "-fls",
            "-fprint0",
        ],
        "git" => &["-c", "--config-env", "--exec-path", "--ext-diff"],
        name if name.starts_with("python")
            || matches!(
                name,
                "node" | "ruby" | "perl" | "php" | "deno" | "bun" | "lua"
            ) =>
        {
            &["-c", "-e", "-E", "-p", "-r", "--eval", "--print"]
        }
        _ => &[],
    }
}

/// Whether `text` fits `pattern`, where `*` stands for any text, spaces included.
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

/// One command against one pattern as Claude writes them: `git diff:*` and `git diff *` are `git
/// diff` with or without more words; otherwise `*` is any text.
fn pattern_covers(pattern: &str, command: &str) -> bool {
    let prefix = pattern
        .strip_suffix(":*")
        .or_else(|| pattern.strip_suffix(" *"));
    match prefix {
        Some(prefix) => {
            glob_matches(prefix, command) || glob_matches(&format!("{prefix} *"), command)
        }
        None => glob_matches(pattern, command),
    }
}

/// One simple command, not looking inside it.
fn named(command: &[String], patterns: &[String]) -> bool {
    let Some(program) = command.first() else {
        return false;
    };
    let line = command.join(" ");
    let escapes = escape_options(&program_basename(program));
    patterns.iter().any(|pattern| {
        pattern_covers(pattern, &line)
            && !command[1..].iter().any(|word| {
                let flag = word.split('=').next().unwrap_or(word);
                escapes.contains(&flag) && !pattern.contains(flag)
            })
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
            &["git", "diff", "-c", "core.pager=x"],
            &["git", "diff", "--ext-diff"],
            &["git", "diff", "--config-env=a=b"],
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
        assert!(allowed(&["python3", "-c", "1"], &["python3 -c *"]));
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
