//! The app's own `themis` on every agent run's `PATH`, on every OS.
//!
//! An agent's shell finds `themis` first in `agent-cli/` in the app data, a folder agents cannot
//! write that holds only it. On macOS and Linux that is a link named `themis` to the CLI (the
//! desktop app acts as the CLI when started under that name). On Windows the desktop app is a
//! window program, which cannot serve as a console command, so the installer ships the console
//! `themis` next to it as `themis-cli.exe` (`tauri.windows.conf.json`, staged by
//! `scripts/stage-cli.mjs`), and `agent-cli\themis.exe` is a copy of it, refreshed when it
//! changes (Windows links need privileges a person may not have).
use std::path::{Path, PathBuf};

/// The folder that puts the app's own `themis` first on an agent's `PATH`, or `None` when there
/// is no CLI to put there.
pub(super) fn link_cli(data_dir: &Path, cli: Option<&Path>) -> Option<PathBuf> {
    let folder = data_dir.join("agent-cli");
    #[cfg(unix)]
    {
        let target = cli?.canonicalize().ok()?;
        std::fs::create_dir_all(&folder).ok()?;
        let link = folder.join("themis");
        if std::fs::read_link(&link).ok().as_deref() == Some(target.as_path()) {
            return Some(folder);
        }
        let fresh = folder.join(format!(".themis-{}", uuid::Uuid::new_v4()));
        std::os::unix::fs::symlink(&target, &fresh).ok()?;
        if std::fs::rename(&fresh, &link).is_err() {
            let _ = std::fs::remove_file(&fresh);
            return None;
        }
        Some(folder)
    }
    #[cfg(not(unix))]
    {
        copy_cli(cli?, &folder).then_some(folder)
    }
}

/// Copies `cli` to `folder/themis.exe` unless the copy there is already it (same length and
/// modified time, which a copy keeps). A copy in use by a running command cannot be replaced;
/// the one there then stays until the next start. Whether `folder` now holds a CLI.
#[cfg_attr(unix, allow(dead_code))]
fn copy_cli(cli: &Path, folder: &Path) -> bool {
    let copy = folder.join("themis.exe");
    let stamp = |path: &Path| {
        let metadata = std::fs::metadata(path).ok()?;
        Some((metadata.len(), metadata.modified().ok()?))
    };
    let Some(source) = stamp(cli).filter(|(length, _)| *length > 0) else {
        return false;
    };
    if stamp(&copy) == Some(source) {
        return true;
    }
    if std::fs::create_dir_all(folder).is_err() {
        return false;
    }
    let fresh = folder.join(format!(".themis-{}.exe", uuid::Uuid::new_v4()));
    let copied = std::fs::copy(cli, &fresh).is_ok()
        && filetime_from(cli, &fresh)
        && std::fs::rename(&fresh, &copy).is_ok();
    if !copied {
        let _ = std::fs::remove_file(&fresh);
    }
    copy.is_file()
}

/// Gives `copy` the modified time of `source`, so the next start sees it is current.
#[cfg_attr(unix, allow(dead_code))]
fn filetime_from(source: &Path, copy: &Path) -> bool {
    let Ok(modified) = std::fs::metadata(source).and_then(|metadata| metadata.modified()) else {
        return false;
    };
    std::fs::File::options()
        .write(true)
        .open(copy)
        .and_then(|file| file.set_modified(modified))
        .is_ok()
}

/// The CLI this server's executable provides: itself when it is the `themis` CLI, or (macOS and
/// Linux) the desktop app, which acts as the CLI when started as `themis`; on Windows, the console
/// CLI shipped next to the app.
pub(super) fn own_cli() -> Option<PathBuf> {
    cli_for(&std::env::current_exe().ok()?)
}

fn cli_for(exe: &Path) -> Option<PathBuf> {
    let stem = exe.file_stem()?.to_str()?;
    if stem == "themis" || (cfg!(unix) && stem == "themis-desktop") {
        return Some(exe.to_path_buf());
    }
    // `themis-cli` in an installed app; `themis` in a build folder. Ordinary builds give
    // tauri-build an empty `themis-cli` stand-in, which is never used.
    ["themis-cli", "themis"]
        .into_iter()
        .map(|name| exe.with_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
        .find(|path| std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe(name: &str) -> String {
        format!("{name}{}", std::env::consts::EXE_SUFFIX)
    }

    /// The installed console CLI is found next to the app; an empty stand-in never is.
    #[test]
    fn the_shipped_cli_is_found_next_to_the_app() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join(exe("ThemisCode"));
        std::fs::write(&app, b"app").unwrap();
        assert_eq!(cli_for(&app), None);
        std::fs::write(dir.path().join(exe("themis-cli")), b"").unwrap();
        assert_eq!(cli_for(&app), None, "an empty stand-in is not a CLI");
        std::fs::write(dir.path().join(exe("themis")), b"built").unwrap();
        assert_eq!(cli_for(&app), Some(dir.path().join(exe("themis"))));
        std::fs::write(dir.path().join(exe("themis-cli")), b"shipped").unwrap();
        assert_eq!(cli_for(&app), Some(dir.path().join(exe("themis-cli"))));
        let console = dir.path().join(exe("themis"));
        assert_eq!(cli_for(&console), Some(console.clone()));
    }

    /// The copy is made once, kept while current, and refreshed when the shipped CLI changes.
    #[test]
    fn the_copy_follows_the_shipped_cli() {
        let dir = tempfile::tempdir().unwrap();
        let cli = dir.path().join("themis-cli.exe");
        let folder = dir.path().join("agent-cli");
        assert!(!copy_cli(&cli, &folder), "nothing to copy");
        std::fs::write(&cli, b"").unwrap();
        assert!(!copy_cli(&cli, &folder), "an empty stand-in is not copied");
        std::fs::write(&cli, b"version one").unwrap();
        assert!(copy_cli(&cli, &folder));
        let copy = folder.join("themis.exe");
        assert_eq!(std::fs::read(&copy).unwrap(), b"version one");
        let before = std::fs::metadata(&copy).unwrap().modified().unwrap();
        assert!(copy_cli(&cli, &folder));
        assert_eq!(
            std::fs::metadata(&copy).unwrap().modified().unwrap(),
            before
        );
        std::fs::write(&cli, b"version two!").unwrap();
        assert!(copy_cli(&cli, &folder));
        assert_eq!(std::fs::read(&copy).unwrap(), b"version two!");
        let names: Vec<_> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["themis.exe"], "no stray copies");
    }
}
