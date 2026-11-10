//! Shell commands that only read, so they run without asking wherever reads do (Custom's
//! built-in read rule; Settings › Permissions › Ask before read tools still asks).
//!
//! The list is deliberately short and strict: a program reached by its bare name (a project
//! script called `./ls` is not `ls`), every argument inside the project (no absolute path, no
//! `..` step, which is also what the file tools allow), and none of the options that make a
//! listed program write a file or start another program (`sed` only as `sed -n 1,40p FILE`:
//! quiet, printing line numbers or ranges and nothing else). A plain `sh -c 'cat a.txt'` (also
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
        "sed" => sed_prints_lines(args),
        _ => false,
    }
}

/// `sed` only when it provably prints: quiet (`-n`, `--quiet`, `--silent`) and every script
/// nothing but `p`, bare or on a line number, `$` or a range of them (`1,40p`, `5p`, `10,$p`,
/// several `-e`). Any other option (`-i`, `-f`, `--in-place`, ...) or command (`w`, `r`, `e`, `s`)
/// keeps the command risk. GNU `sed` reads options after files too, so every word is looked at.
fn sed_prints_lines(args: &[String]) -> bool {
    let mut quiet = false;
    let mut scripts: Vec<&str> = Vec::new();
    let mut operands: Vec<&str> = Vec::new();
    let mut words = args.iter().map(String::as_str);
    while let Some(word) = words.next() {
        match word {
            "--" => operands.extend(words.by_ref()),
            "--quiet" | "--silent" => quiet = true,
            "--expression" => match words.next() {
                Some(script) => scripts.push(script),
                None => return false,
            },
            "-" => operands.push(word),
            _ if word.starts_with("--") => match word.strip_prefix("--expression=") {
                Some(script) => scripts.push(script),
                None => return false,
            },
            _ if word.starts_with('-') => {
                let Some(script) = sed_short_options(&word[1..], &mut quiet, &mut words) else {
                    return false;
                };
                scripts.extend(script);
            }
            _ => operands.push(word),
        }
    }
    if scripts.is_empty() {
        // Without `-e`, the first operand is the script.
        if operands.is_empty() {
            return false;
        }
        scripts.push(operands.remove(0));
    }
    quiet && scripts.iter().all(|script| prints_lines_only(script))
}

/// One cluster of short options (`n`, or `ne` followed by its script, attached or next).
/// `None` for any option that is not known to only read.
fn sed_short_options<'a>(
    cluster: &'a str,
    quiet: &mut bool,
    words: &mut impl Iterator<Item = &'a str>,
) -> Option<Option<&'a str>> {
    for (at, flag) in cluster.char_indices() {
        match flag {
            'n' => *quiet = true,
            // Extended regexes, separate files, unbuffered, NUL-separated lines: still reads.
            'E' | 'r' | 's' | 'u' | 'z' => {}
            'e' => {
                let attached = &cluster[at + 1..];
                return Some(Some(if attached.is_empty() {
                    words.next()?
                } else {
                    attached
                }));
            }
            _ => return None,
        }
    }
    Some(None)
}

/// A script of `p` commands on line addresses only, separated by `;` or newlines.
fn prints_lines_only(script: &str) -> bool {
    script.split([';', '\n']).all(|command| {
        let command = command.trim();
        if command.is_empty() {
            return true;
        }
        let Some(address) = command.strip_suffix('p') else {
            return false;
        };
        let address = address.trim_end();
        if address.is_empty() {
            return true;
        }
        let mut ends = address.split(',');
        let line = |end: Option<&str>| {
            end.map(str::trim).is_some_and(|end| {
                end == "$" || (!end.is_empty() && end.bytes().all(|byte| byte.is_ascii_digit()))
            })
        };
        let first = ends.next();
        let second = ends.next();
        ends.next().is_none() && line(first) && (second.is_none() || line(second))
    })
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

#[cfg(test)]
mod tests {
    use super::read_only_command;
    use proptest::prelude::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn sed_printing_lines_quietly_is_a_read() {
        for words in [
            &[
                "sed",
                "-n",
                "1,40p",
                ".agents/skills/release-checklist/SKILL.md",
            ][..],
            &["sed", "-n", "5p", "a.txt"],
            &["sed", "-n", "10,$p", "a.txt"],
            &["sed", "-n", "$p", "a.txt"],
            &["sed", "-n", "p", "a.txt"],
            &["sed", "-n", "1p;5,9p", "a.txt"],
            &["sed", "-n", "-e", "1p", "-e", "20,30p", "a.txt", "b.txt"],
            &["sed", "-ne", "1,3p", "a.txt"],
            &["sed", "-n", "-e1p", "a.txt"],
            &["sed", "--quiet", "--expression=2p", "a.txt"],
            &["sed", "--silent", "--expression", "2p", "a.txt"],
            &["sed", "-n", "-s", "1p", "a.txt", "b.txt"],
            &["sed", "-nz", "1p", "a.txt"],
            &["sed", "-n", "1, 4 p", "a.txt"],
            &["sed", "-n", "1p", "--", "a.txt"],
            &["sh", "-c", "sed -n 1,40p README.md"],
            &["bash", "-lc", "sed -n '10,$p' README.md"],
        ] {
            assert!(read_only_command(&argv(words)), "{words:?}");
        }
    }

    #[test]
    fn sed_that_could_write_run_or_read_elsewhere_is_not_a_read() {
        for words in [
            &["sed", "1,40p", "a.txt"][..],
            &["sed", "-n", "a.txt"],
            &["sed", "-n"],
            &["sed", "-n", "-i", "1p", "a.txt"],
            &["sed", "-n", "1p", "a.txt", "-i"],
            &["sed", "-ni", "1p", "a.txt"],
            &["sed", "-n", "-i.bak", "1p", "a.txt"],
            &["sed", "-n", "--in-place", "1p", "a.txt"],
            &["sed", "-n", "--in-place=.bak", "1p", "a.txt"],
            &["sed", "-n", "-I", "1p", "a.txt"],
            &["sed", "-n", "-f", "script.sed", "a.txt"],
            &["sed", "-n", "--file=script.sed", "a.txt"],
            &["sed", "-n", "1w out.txt", "a.txt"],
            &["sed", "-n", "1W out.txt", "a.txt"],
            &["sed", "-n", "1r other.txt", "a.txt"],
            &["sed", "-n", "1e date", "a.txt"],
            &["sed", "-n", "e date", "a.txt"],
            &["sed", "-n", "s/a/b/w out.txt", "a.txt"],
            &["sed", "-n", "s/a/b/e", "a.txt"],
            &["sed", "-n", "s/a/b/p", "a.txt"],
            &["sed", "-n", "/re/p", "a.txt"],
            &["sed", "-n", "1!p", "a.txt"],
            &["sed", "-n", "1{p}", "a.txt"],
            &["sed", "-n", "1,p", "a.txt"],
            &["sed", "-n", ",4p", "a.txt"],
            &["sed", "-n", "1,2,3p", "a.txt"],
            &["sed", "-n", "1p;w out.txt", "a.txt"],
            &["sed", "-n", "-e", "1p", "-e", "w out.txt", "a.txt"],
            &["sed", "-n", "-e"],
            &["sed", "-n", "--expression"],
            &["sed", "-n", "--expr=1p", "a.txt"],
            &["sed", "-n", "--debug", "1p", "a.txt"],
            &["sed", "-n", "1p", "/etc/passwd"],
            &["sed", "-n", "1p", "../secret.txt"],
            &["/usr/bin/sed", "-n", "1p", "a.txt"],
            &["sh", "-c", "sed -i s/a/b/ a.txt"],
            &["sh", "-c", "sed -n 1p a.txt > b.txt"],
        ] {
            assert!(!read_only_command(&argv(words)), "{words:?}");
        }
    }

    fn sed_word() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("-n".to_owned()),
            Just("-e".to_owned()),
            Just("1,40p".to_owned()),
            Just("$p".to_owned()),
            Just("a.txt".to_owned()),
            "[0-9$,;pnes -]{0,8}",
        ]
    }

    /// Options that write in place or read a script file.
    fn writing_option() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("-i".to_owned()),
            Just("-i.bak".to_owned()),
            Just("-ni".to_owned()),
            Just("-I".to_owned()),
            Just("--in-place".to_owned()),
            Just("--in-place=.orig".to_owned()),
            Just("-f".to_owned()),
            Just("--file=x.sed".to_owned()),
        ]
    }

    /// Scripts that write, read another file, run a program or change text: the `w`, `W`,
    /// `r`, `R`, `e`, `i`, `a`, `c` commands and `s` with the `w` or `e` flag.
    fn writing_script() -> impl Strategy<Value = String> {
        prop_oneof![
            "[0-9$,;p]{0,4}[wWrRe] ?[a-z.]{0,6}",
            "[0-9$,;p]{0,4}s/[a-z]{0,3}/[a-z]{0,3}/[gp]{0,2}[we][ a-z.]{0,6}",
            "[0-9$,;p]{0,4}[iac] ?[a-z]{0,4}",
        ]
    }

    fn quiet_prints(words: Vec<String>) -> Vec<String> {
        let mut command = vec!["sed".to_owned(), "-n".to_owned()];
        command.extend(words);
        command
    }

    proptest! {
        /// An in-place or script-file option anywhere among otherwise printing words never
        /// leaves a read (GNU `sed` reads options after files too).
        #[test]
        fn sed_with_a_writing_option_is_never_a_read(
            words in prop::collection::vec(sed_word(), 0..6),
            option in writing_option(),
            at in any::<prop::sample::Index>(),
        ) {
            let mut command = quiet_prints(words);
            let at = 1 + at.index(command.len());
            command.insert(at, option);
            prop_assert!(!read_only_command(&command), "{command:?}");
        }

        /// A writing script never leaves a read, as an `-e` value next to printing ones or as
        /// the script operand.
        #[test]
        fn sed_with_a_writing_script_is_never_a_read(
            before in prop::collection::vec(sed_word(), 0..4),
            script in writing_script(),
            after in prop::collection::vec(sed_word(), 0..4),
            as_expression in any::<bool>(),
        ) {
            let command = if as_expression {
                let mut words = before;
                words.extend(["-e".to_owned(), script]);
                words.extend(after);
                quiet_prints(words)
            } else {
                let mut words = vec![script];
                words.extend(after.into_iter().filter(|word| !word.starts_with('-')));
                quiet_prints(words)
            };
            prop_assert!(!read_only_command(&command), "{command:?}");
        }
    }
}
