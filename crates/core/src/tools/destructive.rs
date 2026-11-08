use super::*;

/// `true` when a flag token names one of `longs` (`--name` or `--name=value`)
/// or, as a short-flag cluster such as `-fd`, contains one of `shorts`. Git
/// accepts unambiguous long-option prefixes (`--forc`), so any non-empty prefix
/// of a listed long option counts: over-flagging only costs an extra prompt.
pub(super) fn has_flag(arg: &str, longs: &[&str], shorts: &str) -> bool {
    if let Some(long) = arg.strip_prefix("--") {
        let name = long.split('=').next().unwrap_or("");
        !name.is_empty() && longs.iter().any(|candidate| candidate.starts_with(name))
    } else if let Some(cluster) = arg.strip_prefix('-') {
        cluster.chars().any(|c| shorts.contains(c))
    } else {
        false
    }
}

/// First argument that is not a flag (or a flag's separate value for the
/// listed value-taking global options), with its index.
pub(super) fn first_subcommand<'a>(
    args: &'a [String],
    value_flags: &[&str],
) -> Option<(usize, &'a str)> {
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if value_flags.contains(&arg) {
            index += 2;
        } else if arg.starts_with('-') || arg.starts_with('+') {
            index += 1;
        } else {
            return Some((index, arg));
        }
    }
    None
}

/// Non-flag arguments before a `--` separator.
fn positionals(args: &[String]) -> impl Iterator<Item = &str> {
    args.iter()
        .map(String::as_str)
        .take_while(|arg| *arg != "--")
        .filter(|arg| !arg.starts_with('-'))
}

/// Git's global options that take their value as the next word.
const GIT_VALUE_OPTIONS: &[&str] = &[
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--config-env",
    "--super-prefix",
    "--attr-source",
];

/// A `git` argv (without the program name) from its subcommand on: `-C . push` is `push`.
pub(crate) fn git_from_subcommand(args: &[String]) -> &[String] {
    first_subcommand(args, GIT_VALUE_OPTIONS).map_or(args, |(at, _)| &args[at..])
}

/// Whether a `git` argv (without the program name) is a destructive,
/// history-rewriting or working-tree-discarding variant that must not share a
/// grant or an allow rule with its harmless siblings.
/// A push that overwrites or deletes on the remote: forced, mirrored, pruning, deleting, or a
/// `+ref` / `:ref` refspec.
fn push_destructive(rest: &[String]) -> bool {
    const LONGS: [&str; 6] = [
        "force",
        "force-with-lease",
        "force-if-includes",
        "mirror",
        "delete",
        "prune",
    ];
    rest.iter().any(|a| has_flag(a, &LONGS, "fd"))
        || rest
            .iter()
            .any(|a| a.starts_with(':') || (a.starts_with('+') && a.len() > 1))
}

fn git_destructive(args: &[String]) -> bool {
    let Some((at, sub)) = first_subcommand(args, GIT_VALUE_OPTIONS) else {
        return false;
    };
    let rest = &args[at + 1..];
    let any = |longs: &[&str], shorts: &str| rest.iter().any(|a| has_flag(a, longs, shorts));
    let has_separator = rest.iter().any(|a| a == "--");
    let first_positional = positionals(rest).next();
    match sub {
        "push" => push_destructive(rest),
        "clean" => any(&["force"], "fdxX"),
        "reset" => {
            // A revision moves the branch (and `--hard` discards edits); only a
            // bare reset, `HEAD`, `--keep` or pathspecs after `--` are safe.
            any(&["hard", "merge"], "") || positionals(rest).any(|a| a != "HEAD")
        }
        // A pathspec checkout (`.`, or anything after `--`) overwrites local edits.
        "checkout" => {
            any(&["force", "discard-changes", "force-create"], "fBC")
                || has_separator
                || positionals(rest).any(|a| a == ".")
        }
        "switch" => any(&["force", "discard-changes", "force-create"], "fBC"),
        "restore" => {
            // Restoring the worktree overwrites local edits; only a pure
            // `--staged` restore leaves them alone.
            let staged = rest.iter().any(|a| {
                a.strip_prefix("--").is_some_and(|n| {
                    n.len() >= 3 && "staged".starts_with(n.split('=').next().unwrap_or(""))
                }) || (a.starts_with('-') && !a.starts_with("--") && a.contains('S'))
            });
            let worktree = any(&["worktree"], "W");
            !staged || worktree
        }
        "branch" => any(&["force"], "DfMC"),
        "tag" => any(&["delete", "force"], "df"),
        "commit" => any(&["amend"], ""),
        // Rebasing local commits on top of the remote rewrites them; a forced pull overwrites.
        "pull" => any(&["rebase", "force"], "rf"),
        // Throwing away a merge in progress discards the conflict fixes made so far.
        "merge" => any(&["abort"], ""),
        "stash" => matches!(first_positional, Some("drop" | "clear")),
        "reflog" => matches!(first_positional, Some("expire" | "delete")),
        "worktree" => matches!(first_positional, Some("remove")) && any(&["force"], "f"),
        "gc" => any(&["prune"], ""),
        "rebase" | "update-ref" | "prune" | "filter-branch" | "filter-repo" => true,
        _ => false,
    }
}

/// `npm`-family words that publish, retract or deprecate a package. npm accepts
/// unambiguous command abbreviations (`npm publi`), so a prefix of at least
/// three characters matches.
fn registry_word(arg: &str) -> bool {
    ["publish", "unpublish", "deprecate"]
        .iter()
        .any(|word| arg.len() >= 3 && word.starts_with(arg))
}

/// Ordinary `npm`/`pnpm`/`yarn` subcommands. Once one of these is reached, later
/// arguments are its own operands (`npm test publish` runs the tests), so the
/// registry scan stops there. Words that run another command (`run`, `exec`,
/// `dlx`, `npm`, `workspace`, ...) are deliberately absent: the scan continues
/// through them, so `yarn workspace pkg npm publish` and `npm run publish` (a
/// script that likely publishes) still count.
const NODE_SUBCOMMANDS: &[&str] = &[
    "test",
    "t",
    "tst",
    "install",
    "i",
    "ci",
    "add",
    "remove",
    "rm",
    "uninstall",
    "un",
    "update",
    "up",
    "upgrade",
    "build",
    "start",
    "stop",
    "restart",
    "ls",
    "list",
    "ll",
    "la",
    "outdated",
    "audit",
    "view",
    "info",
    "show",
    "search",
    "init",
    "link",
    "ln",
    "pack",
    "prune",
    "dedupe",
    "why",
    "explain",
    "config",
    "cache",
    "doctor",
    "help",
    "version",
    "whoami",
    "login",
    "logout",
    "rebuild",
    "fund",
    "ping",
    "prefix",
    "root",
    "bin",
    "bugs",
    "docs",
    "edit",
    "explore",
    "star",
    "stars",
    "unstar",
    "completion",
    "import",
    "store",
    "env",
    "patch",
    "licenses",
    "plugin",
    "constraints",
    "lint",
    "format",
    "dev",
    "serve",
    "typecheck",
    "check",
    "clean",
    "set",
    "get",
];

/// Ordinary `cargo` subcommands (and common plugins); see [`NODE_SUBCOMMANDS`].
const CARGO_SUBCOMMANDS: &[&str] = &[
    "build",
    "b",
    "check",
    "c",
    "clean",
    "doc",
    "d",
    "new",
    "init",
    "add",
    "remove",
    "rm",
    "run",
    "r",
    "test",
    "t",
    "bench",
    "update",
    "search",
    "install",
    "uninstall",
    "fmt",
    "clippy",
    "tree",
    "metadata",
    "fix",
    "vendor",
    "package",
    "locate-project",
    "generate-lockfile",
    "fetch",
    "help",
    "version",
    "report",
    "miri",
    "nextest",
    "llvm-cov",
    "expand",
    "watch",
    "audit",
    "deny",
    "outdated",
    "info",
    "fuzz",
    "login",
    "logout",
];

/// Whether a package-manager argv publishes, retracts or deprecates a package.
/// Every positional is scanned (option values such as `--filter pkg` or
/// `--otp 123456` are skipped as unknown words) until an ordinary subcommand is
/// reached, so `cargo test publish` or `npm test -- publish` are harmless while
/// `pnpm --filter pkg publish` and `cargo +nightly publish` are not.
fn registry_publish(program: &str, args: &[String]) -> bool {
    let cargo = program == "cargo";
    let mut after_flag = false;
    for arg in args
        .iter()
        .map(String::as_str)
        .take_while(|arg| *arg != "--")
    {
        if arg.starts_with('-') {
            after_flag = true;
            continue;
        }
        // A word right after a flag may be that flag's value (`npm -w test
        // publish`), so it is never taken as the subcommand that ends the scan.
        let maybe_value = std::mem::replace(&mut after_flag, false);
        let destructive = if cargo {
            matches!(arg, "publish" | "yank")
        } else {
            registry_word(arg)
        };
        if destructive {
            return true;
        }
        let ordinary = if cargo {
            CARGO_SUBCOMMANDS
        } else {
            NODE_SUBCOMMANDS
        };
        if !maybe_value && ordinary.contains(&arg) {
            return false;
        }
    }
    false
}

/// `rm` with a recursive flag (`-r`, `-R`, `--recursive`, or a cluster such as
/// `-rf`) before a `--` separator.
fn rm_recursive(args: &[String]) -> bool {
    args.iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| match arg.strip_prefix("--") {
            Some(long) => long == "recursive",
            None => arg.starts_with('-') && arg[1..].chars().any(|c| c == 'r' || c == 'R'),
        })
}

/// Basename of a program token, lowercased, without a Windows `.exe` suffix.
pub(crate) fn program_basename(program: &str) -> String {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let name = name.to_ascii_lowercase();
    name.strip_suffix(".exe").map(str::to_owned).unwrap_or(name)
}

/// Whether `command` (program followed by arguments) is a destructive or
/// irreversible variant of a known CLI: `git push --force`, `git clean -fd`,
/// `git reset --hard`, `git checkout -- .`, forced checkout/switch/restore,
/// `git branch -D`, `git stash drop`, `git rebase`, `git commit --amend`,
/// recursive `rm` (`rm -rf`), and `publish`/`yank` style registry commands
/// (found even behind value-taking options such as `pnpm --filter pkg publish`,
/// `npm -w x publish` or `yarn npm publish`, but not as an operand of another
/// subcommand such as `cargo test publish`). Such calls get their own grant
/// key, the `destructive` risk class, and are not covered by broad allow
/// rules (for example `["git","push"]` or a `shell` rule without `command`).
pub fn is_destructive_variant(command: &[String]) -> bool {
    // Wrappers (`sh -c`, `sudo`, `env`, `eval`, `cmd /c`, ...) never hide a
    // destructive command; a wrapped script we cannot read counts as one.
    direct_destructive(command)
        || wrapped::wrapped_commands(command)
            .is_none_or(|inner| inner.iter().any(|command| direct_destructive(command)))
        || super::themis_cli::removes_through_themis(command)
}

/// Whether `command` is asked about every time: a destructive variant, or a `themis` call that
/// switches on a hook, an MCP server or a plugin (see [`super::themis_cli`]). Neither is ever
/// covered by "Allow in this chat" or by an allow rule that does not name it.
pub fn asks_every_time(command: &[String]) -> bool {
    is_destructive_variant(command) || super::themis_cli::switches_on_code(command)
}

/// Every command `command` runs, itself first, then those inside wrappers
/// (`sh -c "..."`, `sudo`, `env`, ...); `None` when a wrapped script cannot be
/// read reliably. Deny rules match against each of them.
pub fn commands_run_by(command: &[String]) -> Option<Vec<Vec<String>>> {
    let mut all = vec![command.to_vec()];
    all.extend(wrapped::wrapped_commands(command)?);
    Some(all)
}

fn direct_destructive(command: &[String]) -> bool {
    let Some(program) = command.first() else {
        return false;
    };
    let name = program_basename(program);
    let args = &command[1..];
    match name.as_str() {
        "git" => git_destructive(args),
        "rm" => rm_recursive(args),
        "npm" | "pnpm" | "yarn" | "cargo" => registry_publish(&name, args),
        _ => false,
    }
}

/// Risk class of a `shell` (or `git`) argv: destructive variants
/// ([`is_destructive_variant`]) are [`RiskLevel::Destructive`], a bare `themis`
/// call has its own level ([`super::themis_cli::direct_risk`]), and everything
/// else runs as [`RiskLevel::Execute`].
pub fn shell_risk(command: &[String]) -> RiskLevel {
    if is_destructive_variant(command) {
        RiskLevel::Destructive
    } else {
        super::themis_cli::direct_risk(command).unwrap_or(RiskLevel::Execute)
    }
}
