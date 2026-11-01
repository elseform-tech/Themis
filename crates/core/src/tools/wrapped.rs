//! Commands hidden inside wrappers: `sh -c "git reset --hard"`, `sudo rm -rf x`,
//! `env X=1 git push --force`, `eval`, `cmd /c` and `powershell -Command`.
//! The destructive check and deny rules look at every command a wrapper would
//! run, so wrapping a command never hides it. Scripts are split with a small
//! shell-like tokenizer; anything it cannot read reliably (unbalanced quotes,
//! command substitution inside a quoted word, a variable as the program, very
//! deep nesting) is reported as unreadable so callers can fail closed.
//!
//! Not covered: script files (`sh build.sh`) and other interpreters
//! (`python -c`, `node -e`), whose contents are not shell commands.

use super::program_basename;

/// Wrapper levels followed before giving up (`sudo sh -c "env X=1 bash -c ..."`).
const MAX_DEPTH: usize = 4;
/// Inner commands collected before giving up.
const MAX_COMMANDS: usize = 64;

/// Every command `command` would run through wrappers, outermost first and
/// without `command` itself. Empty for a plain command; `None` when a wrapped
/// script cannot be read reliably.
pub fn wrapped_commands(command: &[String]) -> Option<Vec<Vec<String>>> {
    let mut found = Vec::new();
    collect(command, 0, &mut found)?;
    Some(found)
}

fn collect(command: &[String], depth: usize, found: &mut Vec<Vec<String>>) -> Option<()> {
    let inner = match unwrap_once(command) {
        Unwrapped::Plain => return Some(()),
        Unwrapped::Opaque => return None,
        Unwrapped::Runs(inner) => inner,
    };
    if depth >= MAX_DEPTH {
        return None;
    }
    for command in inner {
        if found.len() >= MAX_COMMANDS {
            return None;
        }
        found.push(command.clone());
        collect(&command, depth + 1, found)?;
    }
    Some(())
}

enum Unwrapped {
    /// Not a wrapper (or a wrapper with nothing to run).
    Plain,
    Runs(Vec<Vec<String>>),
    Opaque,
}

fn unwrap_once(command: &[String]) -> Unwrapped {
    let Some(program) = command.first() else {
        return Unwrapped::Plain;
    };
    let args = &command[1..];
    match program_basename(program).as_str() {
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "mksh" | "ash" | "fish" => shell_script(args),
        "eval" => script(&args.join(" ")),
        "cmd" => after_switch(args, |a| matches!(a, "/c" | "/k" | "/r")),
        "powershell" | "pwsh" => {
            // -EncodedCommand (also -e, -ec, -enc): base64 we do not decode.
            let encoded = |a: &str| a == "-e" || a == "-ec" || "-encodedcommand".starts_with(a);
            if args.iter().any(|a| a.len() > 1 && encoded(&lower(a))) {
                return Unwrapped::Opaque;
            }
            after_switch(args, |a| {
                a == "/command" || (a.len() >= 2 && "-command".starts_with(a))
            })
        }
        "env" => prefix(args, &["-u", "--unset", "-C", "--chdir"], true, false),
        "sudo" | "doas" => prefix(
            args,
            &["-u", "-g", "-C", "-D", "-h", "-p", "-U", "-r", "-t", "-T"],
            false,
            false,
        ),
        "nice" => prefix(args, &["-n", "--adjustment"], false, false),
        "timeout" => prefix(args, &["-s", "--signal", "-k", "--kill-after"], false, true),
        "xargs" => prefix(
            args,
            &["-I", "-L", "-n", "-P", "-d", "-E", "-s", "-a", "--max-args"],
            false,
            false,
        ),
        "stdbuf" => prefix(args, &["-i", "-o", "-e"], false, false),
        "ionice" => prefix(args, &["-c", "-n", "-p"], false, false),
        "caffeinate" => prefix(args, &["-t", "-w"], false, false),
        "nohup" | "time" | "command" | "exec" | "busybox" => prefix(args, &[], false, false),
        _ => Unwrapped::Plain,
    }
}

fn lower(arg: &str) -> String {
    arg.to_ascii_lowercase()
}

/// `sh -c SCRIPT`, including clusters such as `-lc` or `-ec` and fish's
/// `--command`. Without `-c` the shell runs a file or stdin, which is not ours to read.
fn shell_script(args: &[String]) -> Unwrapped {
    match script_argument(args) {
        ScriptArgument::Inline(text) => script(text),
        ScriptArgument::Next(text) => script(text),
        ScriptArgument::None => Unwrapped::Plain,
    }
}

enum ScriptArgument<'a> {
    Inline(&'a str),
    Next(&'a str),
    None,
}

fn script_argument(args: &[String]) -> ScriptArgument<'_> {
    let mut at = 0;
    let mut wants_script = false;
    while let Some(arg) = args.get(at) {
        if arg == "--" {
            at += 1;
            break;
        }
        if let Some(inline) = arg.strip_prefix("--command=") {
            return ScriptArgument::Inline(inline);
        }
        let flag = arg.len() > 1 && (arg.starts_with('-') || arg.starts_with('+'));
        if !flag {
            break;
        }
        if matches!(
            arg.as_str(),
            "-o" | "+o" | "-O" | "+O" | "--rcfile" | "--init-file"
        ) {
            at += 2;
            continue;
        }
        if arg == "--command"
            || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c'))
        {
            wants_script = true;
        }
        at += 1;
    }
    match (wants_script, args.get(at)) {
        (true, Some(text)) => ScriptArgument::Next(text),
        _ => ScriptArgument::None,
    }
}

/// The script of `sh -c SCRIPT` and its kin; `None` for anything else.
pub(super) fn shell_script_text(command: &[String]) -> Option<&str> {
    let (program, args) = command.split_first()?;
    if !matches!(
        program_basename(program).as_str(),
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "mksh" | "ash" | "fish"
    ) {
        return None;
    }
    match script_argument(args) {
        ScriptArgument::Inline(text) | ScriptArgument::Next(text) => Some(text),
        ScriptArgument::None => None,
    }
}

/// Windows-style wrappers: everything after the matching switch is one script.
fn after_switch(args: &[String], is_switch: impl Fn(&str) -> bool) -> Unwrapped {
    match args.iter().position(|a| is_switch(&lower(a))) {
        Some(at) if at + 1 < args.len() => script(&args[at + 1..].join(" ")),
        _ => Unwrapped::Plain,
    }
}

/// Prefix wrappers such as `sudo`, `env` and `timeout`: skip options (and, for
/// `env`, `NAME=value` assignments; for `timeout`, the duration) and run the rest.
fn prefix(args: &[String], value_flags: &[&str], assignments: bool, duration: bool) -> Unwrapped {
    let mut at = 0;
    while let Some(arg) = args.get(at) {
        if arg == "--" {
            at += 1;
            break;
        }
        if assignments && matches!(arg.as_str(), "-S" | "--split-string") {
            return args
                .get(at + 1)
                .map_or(Unwrapped::Plain, |text| script(text));
        }
        if assignments && is_assignment(arg) {
            at += 1;
        } else if value_flags.contains(&arg.as_str()) {
            at += 2;
        } else if arg.len() > 1 && arg.starts_with('-') {
            at += 1;
        } else {
            break;
        }
    }
    if duration && at < args.len() {
        at += 1;
    }
    match args.get(at..) {
        Some(rest) if !rest.is_empty() => Unwrapped::Runs(vec![rest.to_vec()]),
        _ => Unwrapped::Plain,
    }
}

pub(super) fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
    })
}

fn script(text: &str) -> Unwrapped {
    match split_script(text) {
        Some(commands) if commands.is_empty() => Unwrapped::Plain,
        Some(commands) => Unwrapped::Runs(commands),
        None => Unwrapped::Opaque,
    }
}

/// Words that start a line of shell grammar rather than name a program.
const KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "do", "done", "while", "until", "!", "{", "}",
];

/// Split a shell script into simple commands. Separators are newlines, `;`,
/// `&`, `|`, parentheses, backticks and `$(`, so subshells and command
/// substitutions become commands of their own. `None` when the script cannot
/// be read reliably.
pub(super) fn split_script(text: &str) -> Option<Vec<Vec<String>>> {
    split_script_with(text, false)
}

/// [`split_script`], leaving a command's leading `NAME=value` words in place, for a caller that
/// cares what they change.
pub(super) fn split_script_keeping_assignments(text: &str) -> Option<Vec<Vec<String>>> {
    split_script_with(text, true)
}

fn split_script_with(text: &str, keep_assignments: bool) -> Option<Vec<Vec<String>>> {
    let finish = |command: &mut Vec<String>, commands: &mut Vec<Vec<String>>| {
        finish(command, commands, keep_assignments)
    };
    let mut commands = Vec::new();
    let mut command: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = text.chars().peekable();
    let mut previous = ' ';
    let end_word = |word: &mut String, in_word: &mut bool, command: &mut Vec<String>| {
        if *in_word {
            command.push(std::mem::take(word));
            *in_word = false;
        }
    };
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\r' => end_word(&mut word, &mut in_word, &mut command),
            '&' if matches!(previous, '>' | '<') => {
                word.push(c);
                in_word = true;
            }
            '\n' | ';' | '&' | '|' | '(' | ')' | '`' => {
                end_word(&mut word, &mut in_word, &mut command);
                finish(&mut command, &mut commands)?;
            }
            '$' if chars.peek() == Some(&'(') => {
                chars.next();
                end_word(&mut word, &mut in_word, &mut command);
                finish(&mut command, &mut commands)?;
            }
            '#' if !in_word => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        break;
                    }
                }
                finish(&mut command, &mut commands)?;
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        other => word.push(other),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => word.push(chars.next()?),
                        // A command substitution inside a quoted word runs a
                        // command we would have to parse again; refuse instead.
                        '`' => return None,
                        '$' if chars.peek() == Some(&'(') => return None,
                        other => word.push(other),
                    }
                }
            }
            '\\' => match chars.next() {
                Some('\n') | None => {}
                Some(other) => {
                    word.push(other);
                    in_word = true;
                }
            },
            other => {
                word.push(other);
                in_word = true;
            }
        }
        previous = c;
    }
    end_word(&mut word, &mut in_word, &mut command);
    finish(&mut command, &mut commands)?;
    Some(commands)
}

/// Close one simple command: drop leading keywords and `NAME=value`
/// assignments; refuse a program that is only known at run time (`$CMD`).
fn finish(
    command: &mut Vec<String>,
    commands: &mut Vec<Vec<String>>,
    keep_assignments: bool,
) -> Option<()> {
    let words = std::mem::take(command);
    let start = words
        .iter()
        .position(|w| !KEYWORDS.contains(&w.as_str()) && (keep_assignments || !is_assignment(w)))
        .unwrap_or(words.len());
    let words = &words[start..];
    if let Some(program) = words.first() {
        if program.contains('$') {
            return None;
        }
        commands.push(words.to_vec());
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    fn inner(words: &[&str]) -> Option<Vec<Vec<String>>> {
        wrapped_commands(&argv(words))
    }

    #[test]
    fn plain_commands_have_nothing_inside() {
        assert_eq!(inner(&["git", "status"]), Some(vec![]));
        assert_eq!(inner(&["sh", "build.sh"]), Some(vec![]));
        assert_eq!(inner(&["sudo"]), Some(vec![]));
        assert_eq!(inner(&[]), Some(vec![]));
    }

    #[test]
    fn shell_scripts_are_split_into_commands() {
        assert_eq!(
            inner(&["sh", "-c", "git status && git reset --hard; echo 'a b'"]),
            Some(vec![
                argv(&["git", "status"]),
                argv(&["git", "reset", "--hard"]),
                argv(&["echo", "a b"]),
            ])
        );
        assert_eq!(
            inner(&["/bin/bash", "-lc", "X=1 rm -rf build 2>&1 | tee log"]),
            Some(vec![
                argv(&["rm", "-rf", "build", "2>&1"]),
                argv(&["tee", "log"]),
            ])
        );
        assert_eq!(
            inner(&[
                "zsh",
                "-o",
                "errexit",
                "-c",
                "cd x\nif true; then git clean -fd; fi"
            ]),
            Some(vec![
                argv(&["cd", "x"]),
                argv(&["true"]),
                argv(&["git", "clean", "-fd"]),
            ])
        );
        assert_eq!(
            inner(&["fish", "--command=rm -r x"]),
            Some(vec![argv(&["rm", "-r", "x"])])
        );
    }

    #[test]
    fn substitutions_and_subshells_become_commands() {
        assert_eq!(
            inner(&[
                "sh",
                "-c",
                "echo $(git push --force) `rm -r x` (git stash drop)"
            ]),
            Some(vec![
                argv(&["echo"]),
                argv(&["git", "push", "--force"]),
                argv(&["rm", "-r", "x"]),
                argv(&["git", "stash", "drop"]),
            ])
        );
    }

    #[test]
    fn prefix_wrappers_run_the_rest() {
        assert_eq!(
            inner(&["sudo", "-u", "me", "rm", "-rf", "/"]),
            Some(vec![argv(&["rm", "-rf", "/"])])
        );
        assert_eq!(
            inner(&["env", "-u", "X", "A=1", "git", "push", "-f"]),
            Some(vec![argv(&["git", "push", "-f"])])
        );
        assert_eq!(
            inner(&["timeout", "-s", "KILL", "10", "git", "clean", "-fdx"]),
            Some(vec![argv(&["git", "clean", "-fdx"])])
        );
        assert_eq!(
            inner(&["nohup", "nice", "-n", "5", "sh", "-c", "rm -r x"]),
            Some(vec![
                argv(&["nice", "-n", "5", "sh", "-c", "rm -r x"]),
                argv(&["sh", "-c", "rm -r x"]),
                argv(&["rm", "-r", "x"]),
            ])
        );
        assert_eq!(
            inner(&["env", "-S", "git reset --hard"]),
            Some(vec![argv(&["git", "reset", "--hard"])])
        );
        assert_eq!(
            inner(&["xargs", "-I", "{}", "rm", "-rf", "{}"]),
            Some(vec![argv(&["rm", "-rf", "{}"])])
        );
    }

    #[test]
    fn windows_and_eval_wrappers_are_read() {
        assert_eq!(
            inner(&["cmd", "/C", "git", "reset", "--hard", "&&", "dir"]),
            Some(vec![argv(&["git", "reset", "--hard"]), argv(&["dir"])])
        );
        assert_eq!(
            inner(&["pwsh", "-Command", "git push --force; ls"]),
            Some(vec![argv(&["git", "push", "--force"]), argv(&["ls"])])
        );
        assert_eq!(
            inner(&["powershell", "-EncodedCommand", "ZQBjAGgAbwA="]),
            None
        );
        assert_eq!(
            inner(&["eval", "git", "branch", "-D", "x"]),
            Some(vec![argv(&["git", "branch", "-D", "x"])])
        );
    }

    #[test]
    fn unreadable_scripts_are_reported() {
        assert_eq!(inner(&["sh", "-c", "echo 'unclosed"]), None);
        assert_eq!(inner(&["sh", "-c", "echo \"$(git reset --hard)\""]), None);
        assert_eq!(inner(&["sh", "-c", "$GIT reset --hard"]), None);
        let deep = argv(&["sudo", "sudo", "sudo", "sudo", "sudo", "rm", "-rf", "x"]);
        assert_eq!(wrapped_commands(&deep), None);
    }

    #[test]
    fn quoting_keeps_separators_inside_words() {
        assert_eq!(
            inner(&["sh", "-c", r#"echo "a; rm -rf x" 'b && c' d\;e"#]),
            Some(vec![argv(&["echo", "a; rm -rf x", "b && c", "d;e"])])
        );
        assert_eq!(
            inner(&["sh", "-c", "ls # ; rm -rf x\npwd"]),
            Some(vec![argv(&["ls"]), argv(&["pwd"])])
        );
    }

    #[test]
    fn wrapped_destructive_commands_are_destructive() {
        use crate::tools::is_destructive_variant as destructive;
        for danger in [
            &["sh", "-c", "git reset --hard"][..],
            &["bash", "-lc", "cd x && git push --force origin main"],
            &["sudo", "rm", "-rf", "/tmp/x"],
            &["env", "GIT_DIR=.git", "git", "clean", "-fdx"],
            &["timeout", "5", "sh", "-c", "git stash drop"],
            &["sh", "-c", "echo $(git branch -D x)"],
            &["cmd", "/c", "git", "reset", "--hard"],
            // Unreadable scripts count as destructive.
            &["sh", "-c", "echo 'unclosed"],
            &["sh", "-c", "$CMD"],
        ] {
            assert!(destructive(&argv(danger)), "{danger:?}");
        }
        for harmless in [
            &["sh", "-c", "git status && cargo test"][..],
            &["bash", "-c", "echo 'git reset --hard'"],
            &["sudo", "ls"],
            &["env", "X=1", "cargo", "build"],
            &["sh", "build.sh"],
            &["xargs", "rm"],
        ] {
            assert!(!destructive(&argv(harmless)), "{harmless:?}");
        }
    }

    proptest! {
        #[test]
        fn never_panics_and_stays_bounded(words in proptest::collection::vec(".{0,24}", 0..6)) {
            if let Some(found) = wrapped_commands(&words) {
                prop_assert!(found.len() <= MAX_COMMANDS);
                prop_assert!(found.iter().all(|command| !command.is_empty()));
            }
        }

        #[test]
        fn a_command_joined_with_separators_is_found(
            first in "[a-z]{1,8}( [a-z-]{1,6}){0,3}",
            second in "[a-z]{1,8}( [a-z-]{1,6}){0,3}",
            separator in prop_oneof![Just("; "), Just(" && "), Just(" || "), Just(" | "), Just("\n")],
        ) {
            prop_assume!(!KEYWORDS.contains(&first.split(' ').next().unwrap()));
            prop_assume!(!KEYWORDS.contains(&second.split(' ').next().unwrap()));
            let text = format!("{first}{separator}{second}");
            let found = wrapped_commands(&argv(&["sh", "-c", &text])).unwrap();
            let words = |s: &str| s.split(' ').map(str::to_owned).collect::<Vec<_>>();
            prop_assert_eq!(found.first(), Some(&words(&first)));
            prop_assert!(found.contains(&words(&second)));
        }
    }
}
