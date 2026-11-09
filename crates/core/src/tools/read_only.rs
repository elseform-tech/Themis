//! Shell commands that only read, so they run without asking wherever reads do (Custom's
//! built-in read rule; Settings › Permissions › Ask before read tools still asks).
//!
//! The list is deliberately short and strict: a program reached by its bare name (a project
//! script called `./ls` is not `ls`), every argument inside the project (no absolute path, no
//! `..` step, which is also what the file tools allow), and none of the options that make a
//! listed program write a file or start another program. A plain `sh -c 'cat a.txt'` (also
//! `bash`, `zsh` or `dash`, with `-c`, `-lc` or `-cl`) is judged by the one command inside it
//! when the script is that command in plain words (see `wrapped::plain_shell_command`); any
//! other wrapper, a pipeline, a list, a redirect, a substitution or a glob keeps the ordinary
//! command risk and is asked about. Secret files are still asked about through the
//! sensitive-file check, which reads the same arguments, inside wrappers too.

use super::destructive::has_flag;

/// Whether `command` (program followed by arguments) only reads inside the project.
#[must_use]
pub fn read_only_command(command: &[String]) -> bool {
    if let Some(inner) = super::wrapped::plain_shell_command(command) {
        return read_only_command(&inner);
    }
    let Some((program, args)) = command.split_first() else {
        return false;
    };
    if program.is_empty() || program.contains(['/', '\\']) {
        return false;
    }
    if !args.iter().all(|argument| stays_inside(argument)) {
        return false;
    }
    let any = |longs: &[&str], shorts: &str| args.iter().any(|a| has_flag(a, longs, shorts));
    match program.as_str() {
        "ls" | "pwd" | "wc" | "head" | "tail" | "cat" | "stat" | "du" | "diff" | "cmp"
        | "which" | "grep" | "egrep" | "fgrep" => true,
        // `file -C` compiles a magic file; `tree -o` writes its listing to a file.
        "file" => !any(&["compile"], "C"),
        "tree" => !args.iter().any(|a| a == "-o"),
        // `--pre` runs a program on each file; `-z` runs decompressors.
        "rg" => !any(&["pre", "pre-glob", "search-zip"], "z"),
        "find" => !args.iter().any(|a| {
            matches!(
                a.as_str(),
                "-exec"
                    | "-execdir"
                    | "-ok"
                    | "-okdir"
                    | "-delete"
                    | "-fprint"
                    | "-fprint0"
                    | "-fprintf"
                    | "-fls"
            )
        }),
        // `-o` writes the result to a file; `--compress-program` runs one.
        "sort" => !any(&["output", "compress-program"], "o"),
        "git" => super::argv::git_read_only(command),
        _ => false,
    }
}

/// An argument that names nothing outside the project: no rooted path and no `..` step, in the
/// argument itself, in a long option's value (`--file=/etc/x`) or in a short option's attached
/// value (`-f/etc/x`).
fn stays_inside(argument: &str) -> bool {
    let value = if let Some(long) = argument.strip_prefix("--") {
        long.split_once('=').map_or("", |(_, value)| value)
    } else if let Some(short) = argument.strip_prefix('-') {
        short.get(1..).unwrap_or("")
    } else {
        argument
    };
    let escapes =
        super::argv::rooted_path(value) || value.split(['/', '\\']).any(|part| part == "..");
    !escapes
}
