//! A plugin's whole folder, when it doesn't fit in the registry.
//!
//! The registry is one JSON file read whole on every list, so it only holds a plugin's text when
//! all of it is small. A plugin over those bounds, or with a large or binary file (a compiled
//! hook, say), keeps its components in the registry and everything else here, in a
//! content-named folder of its scope's plugin directory. Nothing is held in memory: files are
//! listed first, hashed and copied one at a time. A check only hashes, so scanning a catalog
//! stores nothing; the files are kept when a package is really installed.

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The largest single file a package keeps. The largest in the official catalog is 9.2 MiB.
pub(super) const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// The most a package may hold. The largest in the official catalog is 207 MiB.
pub(super) const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
/// The most files a package may hold. The most in the official catalog is 7,927.
pub(super) const MAX_FILES: usize = 20_000;
/// The largest a repository may grow while it is downloaded, its history included.
pub(super) const MAX_CLONE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

// The limits stay well clear of what the official catalog needs (hyperframes, salesforce-development).
const _: () = assert!(MAX_TOTAL_BYTES >= 2 * 207 * 1024 * 1024);
const _: () = assert!(MAX_FILES >= 2 * 7927);
const _: () = assert!(MAX_FILE_BYTES >= 4 * 9 * 1024 * 1024);

/// What fits in the registry: a file, and a plugin's text in all.
pub(super) const MAX_INLINE_FILE_BYTES: u64 = 1024 * 1024;
pub(super) const MAX_INLINE_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_INLINE_FILES: usize = 1000;

/// Where a package keeps its files within a scope's plugin directory.
pub(super) const PACKAGES_DIR: &str = "packages";
/// Written last when a package has been copied out, so a later run needn't look at every file.
const COPIED_MARKER: &str = ".themis-package";
/// A folder younger than this may belong to an install still on its way to the registry.
const PRUNE_AFTER: std::time::Duration = std::time::Duration::from_secs(3600);

/// The registry's record of a package kept on disk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PackageRef {
    /// SHA-256 over every file's path, executable bit and content.
    pub hash: String,
    pub files: usize,
    pub bytes: u64,
}

impl PackageRef {
    pub(super) fn validate(&self) -> anyhow::Result<()> {
        if !valid_hash(&self.hash) {
            bail!("Invalid plugin package");
        }
        Ok(())
    }
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// One file found in a plugin's folder; nothing of it has been read.
#[derive(Debug)]
pub(super) struct Entry {
    pub(super) relative: String,
    pub(super) path: PathBuf,
    pub(super) bytes: u64,
    pub(super) executable: bool,
}

/// The bounds a walk keeps to.
#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub(super) file_bytes: u64,
    pub(super) total_bytes: u64,
    pub(super) files: usize,
}

pub(super) const LIMITS: Limits = Limits {
    file_bytes: MAX_FILE_BYTES,
    total_bytes: MAX_TOTAL_BYTES,
    files: MAX_FILES,
};

/// The hash of `entries`, sorted by path. It reads one file at a time.
pub(super) fn digest(entries: &[Entry]) -> anyhow::Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    for entry in entries {
        hash.update(entry.relative.as_bytes());
        hash.update([0, u8::from(entry.executable)]);
        hash.update(entry.bytes.to_le_bytes());
        let mut file = fs::File::open(&entry.path)?;
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hash.update(&buffer[..read]);
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// The record for `entries`, with their hash.
pub(super) fn describe(entries: &[Entry]) -> anyhow::Result<PackageRef> {
    Ok(PackageRef {
        hash: digest(entries)?,
        files: entries.len(),
        bytes: entries.iter().map(|entry| entry.bytes).sum(),
    })
}

fn set_mode(path: &Path, executable: bool) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Only ever these: a package never brings set-id bits with it.
        let mode = if executable { 0o755 } else { 0o644 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = (path, executable);
    Ok(())
}

/// Keeps `entries` in `store` under their hash. The folder appears whole or not at all.
pub(super) fn keep(entries: &[Entry], package: &PackageRef, store: &Path) -> anyhow::Result<()> {
    package.validate()?;
    let target = store.join(&package.hash);
    if target.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(store)?;
    let temporary = store.join(format!(".tmp-{}", super::revision()));
    let copied = (|| {
        for entry in entries {
            let path = temporary.join(&entry.relative);
            fs::create_dir_all(path.parent().context("Missing parent")?)?;
            fs::copy(&entry.path, &path)?;
            set_mode(&path, entry.executable)?;
        }
        // Another install of the same package may have got here first; either is complete.
        match fs::rename(&temporary, &target) {
            Err(_) if target.is_dir() => Ok(()),
            result => result.map_err(anyhow::Error::from),
        }
    })();
    if temporary.exists() {
        let _ = fs::remove_dir_all(&temporary);
    }
    copied?;
    crate::diagnostics::emit(
        "plugins",
        "package_kept",
        "info",
        serde_json::json!({"files": package.files, "bytes": package.bytes}),
    );
    Ok(())
}

/// Copies a kept package to `directory`, once per folder. `false` when it is no longer kept.
pub(super) fn copy_out(
    store: &Path,
    package: &PackageRef,
    directory: &Path,
    prepare_parent: &dyn Fn(&Path) -> anyhow::Result<()>,
) -> anyhow::Result<bool> {
    package.validate()?;
    let source = store.join(&package.hash);
    let marker = directory.join(COPIED_MARKER);
    if fs::read_to_string(&marker).is_ok_and(|text| text == package.hash) {
        return Ok(true);
    }
    if !source.is_dir() {
        return Ok(false);
    }
    let mut stack = vec![source.clone()];
    while let Some(folder) = stack.pop() {
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            let from = entry.path();
            if entry.file_type()?.is_dir() {
                stack.push(from);
                continue;
            }
            let to = directory.join(from.strip_prefix(&source)?);
            prepare_parent(to.parent().context("Missing parent")?)?;
            if to
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                bail!("Symlink resource target rejected");
            }
            // A persistent MCP process may be reading this revision: replace, never rewrite.
            let temporary = to.with_extension(format!("{}.tmp", super::revision()));
            let executable = entry.metadata()?.permissions();
            fs::copy(&from, &temporary)?;
            fs::set_permissions(&temporary, executable)?;
            fs::rename(&temporary, &to)?;
        }
    }
    prepare_parent(directory)?;
    fs::write(marker, &package.hash)?;
    Ok(true)
}

/// Removes kept packages nothing refers to any more.
pub(super) fn prune(store: &Path, referenced: &BTreeSet<&str>) {
    let Ok(folders) = fs::read_dir(store) else {
        return;
    };
    for folder in folders.flatten() {
        let name = folder.file_name();
        let name = name.to_string_lossy();
        let old_enough = folder
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= PRUNE_AFTER);
        if old_enough && !referenced.contains(name.as_ref()) {
            let _ = fs::remove_dir_all(folder.path());
        }
    }
}

/// The size of a folder's files, however deep. Links aren't followed.
pub(super) fn tree_bytes(root: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(folder) = stack.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(entry.path());
            } else {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(folder: &Path, relative: &str, content: &[u8], executable: bool) -> Entry {
        let path = folder.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        Entry {
            relative: relative.to_owned(),
            path,
            bytes: content.len() as u64,
            executable,
        }
    }

    #[test]
    fn a_kept_package_comes_back_byte_for_byte_with_its_mode() {
        let folder = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let entries = vec![
            entry(folder.path(), "bin/tool", &[0xff, 0x00, 0x80], true),
            entry(folder.path(), "notes/a.txt", b"hello", false),
        ];
        let package = describe(&entries).unwrap();
        assert_eq!((package.files, package.bytes), (2, 8));
        keep(&entries, &package, store.path()).unwrap();
        // Keeping it again changes nothing, and leaves no temporary folder.
        keep(&entries, &package, store.path()).unwrap();
        assert_eq!(fs::read_dir(store.path()).unwrap().count(), 1);
        let target = out.path().join("plugin");
        let prepare = |parent: &Path| Ok(fs::create_dir_all(parent)?);
        assert!(copy_out(store.path(), &package, &target, &prepare).unwrap());
        assert_eq!(
            fs::read(target.join("bin/tool")).unwrap(),
            [0xff, 0x00, 0x80]
        );
        assert_eq!(fs::read(target.join("notes/a.txt")).unwrap(), b"hello");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &str| {
                fs::metadata(target.join(path))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777
            };
            assert_eq!((mode("bin/tool"), mode("notes/a.txt")), (0o755, 0o644));
        }
        // Copied once: a later run doesn't touch the files again.
        fs::remove_file(target.join("notes/a.txt")).unwrap();
        assert!(copy_out(store.path(), &package, &target, &prepare).unwrap());
        assert!(!target.join("notes/a.txt").exists());
    }

    #[test]
    fn the_hash_binds_paths_modes_and_contents() {
        let folder = tempfile::tempdir().unwrap();
        let hash = |path: &str, content: &[u8], executable: bool| {
            digest(&[entry(folder.path(), path, content, executable)]).unwrap()
        };
        let base = hash("a", b"x", false);
        assert_eq!(hash("a", b"x", false), base);
        assert_ne!(hash("b", b"x", false), base);
        assert_ne!(hash("a", b"y", false), base);
        assert_ne!(hash("a", b"x", true), base);
    }

    #[test]
    fn a_package_that_is_gone_is_reported_and_a_bad_hash_is_refused() {
        let store = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let prepare = |parent: &Path| Ok(fs::create_dir_all(parent)?);
        let gone = PackageRef {
            hash: "a".repeat(64),
            files: 1,
            bytes: 1,
        };
        assert!(!copy_out(store.path(), &gone, out.path(), &prepare).unwrap());
        for bad in ["", "../../etc", &"g".repeat(64), &"a".repeat(63)] {
            let package = PackageRef {
                hash: (*bad).to_owned(),
                files: 0,
                bytes: 0,
            };
            assert!(package.validate().is_err(), "{bad}");
            assert!(copy_out(store.path(), &package, out.path(), &prepare).is_err());
        }
    }

    #[test]
    fn pruning_leaves_what_is_referenced_and_what_is_new() {
        let store = tempfile::tempdir().unwrap();
        fs::create_dir(store.path().join("kept")).unwrap();
        fs::create_dir(store.path().join("new")).unwrap();
        prune(store.path(), &BTreeSet::from(["kept"]));
        assert!(store.path().join("kept").is_dir() && store.path().join("new").is_dir());
    }
}
