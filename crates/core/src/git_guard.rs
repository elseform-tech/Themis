//! Git the app runs for itself runs outside the sandbox, in a repository whose configuration the
//! agent's shell or a downloaded project may have written. That configuration can name programs,
//! so the app's own listings turn those off: they must start nothing but Git.

/// Turns off what starts a program whatever the command: goes before the subcommand.
pub const NO_MONITOR: [&str; 2] = ["-c", "core.fsmonitor=false"];

/// What `git diff` needs on top: no external diff, whichever way it is named, and no text
/// conversion. A submodule's dirt isn't asked about, since that runs Git under its configuration.
pub const DIFF_FLAGS: [&str; 3] = [
    "--no-ext-diff",
    "--no-textconv",
    "--ignore-submodules=dirty",
];

/// The same for `git status`.
pub const STATUS_FLAGS: [&str; 1] = ["--ignore-submodules=dirty"];

/// Lists the content filters the configuration defines, with where from, for [`filters_off`].
pub const LIST_FILTERS: [&str; 7] = [
    "config",
    "--show-scope",
    "--includes",
    "-z",
    "--name-only",
    "--get-regexp",
    "^filter\\.",
];

/// Most filters one repository may define before its listings are refused.
const MAX_FILTERS: usize = 64;

/// The `-c` options that turn off the filters in `listing` (the output of [`LIST_FILTERS`]). Those
/// of the person's own global or system configuration stay: Git LFS lives there. `None` when one
/// can't be turned off this way, or there are too many: then the listing isn't run.
#[must_use]
pub fn filters_off(listing: &str) -> Option<Vec<String>> {
    let mut drivers: Vec<&str> = Vec::new();
    let mut fields = listing.split('\0');
    while let (Some(scope), Some(key)) = (fields.next(), fields.next()) {
        if matches!(scope, "system" | "global" | "command") {
            continue;
        }
        // `filter.<name>.<variable>`: a name may hold dots, and a bare `filter.x` holds nothing.
        let Some((name, _)) = key
            .strip_prefix("filter.")
            .and_then(|rest| rest.rsplit_once('.'))
        else {
            continue;
        };
        if name.is_empty() || name.contains('=') || name.chars().any(char::is_control) {
            return None;
        }
        if !drivers.contains(&name) {
            drivers.push(name);
        }
    }
    if drivers.len() > MAX_FILTERS {
        return None;
    }
    let mut options = Vec::new();
    for name in drivers {
        for setting in [
            format!("filter.{name}.clean="),
            format!("filter.{name}.smudge="),
            format!("filter.{name}.process="),
            format!("filter.{name}.required=false"),
        ] {
            options.push("-c".to_owned());
            options.push(setting);
        }
    }
    Some(options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn the_repositorys_filters_go_off_and_the_persons_stay() {
        let listing = "local\0filter.x.clean\0local\0filter.x.required\0global\0filter.lfs.clean\0worktree\0filter.a.b.smudge\0";
        let options = filters_off(listing).unwrap();
        assert!(options.contains(&"filter.x.clean=".to_owned()));
        assert!(options.contains(&"filter.x.required=false".to_owned()));
        assert!(options.contains(&"filter.a.b.process=".to_owned()));
        assert!(!options.iter().any(|option| option.contains("lfs")));
        // Each once.
        assert_eq!(
            options.iter().filter(|o| *o == "filter.x.clean=").count(),
            1
        );
    }

    #[test]
    fn a_filter_that_cannot_be_turned_off_stops_the_listing() {
        assert_eq!(filters_off("local\0filter.a=b.clean\0"), None);
        assert_eq!(filters_off("local\0filter.a\nb.clean\0"), None);
        let many: String = (0..=MAX_FILTERS)
            .map(|index| format!("local\0filter.f{index}.clean\0"))
            .collect();
        assert_eq!(filters_off(&many), None);
        assert_eq!(filters_off(""), Some(Vec::new()));
    }

    proptest! {
        /// Whatever Git prints: no panic, and every option turns a setting off.
        #[test]
        fn any_listing_reads_safely(listing in "\\PC{0,200}", raw in "[a-z.=\\x00]{0,80}") {
            for text in [listing.as_str(), raw.as_str()] {
                for option in filters_off(text).unwrap_or_default() {
                    prop_assert!(option == "-c" || option.ends_with('=') || option.ends_with("=false"));
                }
            }
        }
    }
}
