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

/// Keeps `entries` in `store` under their hash. The folder appears whole or not at all, and one
/// already there counts only if it really is this package: a project's store is in the project.
pub(super) fn keep(entries: &[Entry], package: &PackageRef, store: &Path) -> anyhow::Result<()> {
    if verified(store, package)?.is_some() {
        return Ok(());
    }
    let target = store.join(&package.hash);
    remove_any(&target)?;
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

/// Removes a file, a link (not what it points at) or a folder, if any is there.
fn remove_any(path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path)?,
        Ok(_) => fs::remove_file(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn is_executable(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

/// A kept package's files, if what is stored really is the package: every file hashed, no links,
/// nothing extra. `None` when it is gone or isn't.
pub(super) fn verified(store: &Path, package: &PackageRef) -> anyhow::Result<Option<Vec<Entry>>> {
    package.validate()?;
    let root = store.join(&package.hash);
    if !fs::symlink_metadata(&root).is_ok_and(|metadata| metadata.is_dir()) {
        return Ok(None);
    }
    let mut entries = Vec::new();
    let mut stack = vec![(root, String::new())];
    while let Some((folder, logical)) = stack.pop() {
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if logical.is_empty() {
                name
            } else {
                format!("{logical}/{name}")
            };
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.is_dir() {
                stack.push((entry.path(), relative));
            } else if metadata.is_file() {
                entries.push(Entry {
                    relative,
                    path: entry.path(),
                    bytes: metadata.len(),
                    executable: is_executable(&metadata),
                });
            } else {
                return Ok(None);
            }
        }
    }
    entries.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok((describe(&entries)? == *package).then_some(entries))
}

/// Whether two files hold the same bytes, reading one piece of each at a time.
fn same_content(a: &Path, b: &Path) -> anyhow::Result<bool> {
    let (mut a, mut b) = (fs::File::open(a)?, fs::File::open(b)?);
    let (mut left, mut right) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    loop {
        let read = a.read(&mut left)?;
        if read == 0 {
            return Ok(b.read(&mut right)? == 0);
        }
        if b.read_exact(&mut right[..read]).is_err() || left[..read] != right[..read] {
            return Ok(false);
        }
    }
}

/// Whether what is at `path` is this entry: a real file, not a link, with its bytes and its bit.
fn matches(path: &Path, entry: &Entry) -> anyhow::Result<bool> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(false);
    };
    Ok(metadata.is_file()
        && metadata.len() == entry.bytes
        && is_executable(&metadata) == entry.executable
        && same_content(path, &entry.path)?)
}

/// The paths of `entries`, for [`remove_others`].
pub(super) fn names(entries: &[Entry]) -> BTreeSet<String> {
    entries.iter().map(|entry| entry.relative.clone()).collect()
}

/// Removes everything in `directory` that isn't one of `wanted` or a folder leading to one. Links
/// are removed as links, never followed. The project's copy of a plugin is written to by whoever
/// can write the project, so nothing found there is taken on trust.
pub(super) fn remove_others(directory: &Path, wanted: &BTreeSet<String>) -> anyhow::Result<()> {
    let metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() {
        bail!("Plugin files folder is not a folder");
    }
    let folders: BTreeSet<&str> = wanted
        .iter()
        .flat_map(|path| {
            path.char_indices()
                .filter(|(_, character)| *character == '/')
                .map(|(at, _)| &path[..at])
        })
        .collect();
    let mut stack = vec![(directory.to_path_buf(), String::new())];
    while let Some((folder, logical)) = stack.pop() {
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if logical.is_empty() {
                name
            } else {
                format!("{logical}/{name}")
            };
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.is_dir() && folders.contains(relative.as_str()) {
                stack.push((entry.path(), relative));
            } else if !(metadata.is_file() && wanted.contains(&relative)) {
                remove_any(&entry.path())?;
            }
        }
    }
    Ok(())
}

/// Makes `directory` hold each of `entries`, replacing only what differs: a persistent MCP process
/// may be reading the rest.
pub(super) fn copy_to(
    entries: &[Entry],
    directory: &Path,
    prepare_parent: &dyn Fn(&Path) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if fs::symlink_metadata(directory).is_ok_and(|metadata| !metadata.is_dir()) {
        bail!("Plugin files folder is not a folder");
    }
    prepare_parent(directory)?;
    remove_others(directory, &names(entries))?;
    for entry in entries {
        let to = directory.join(&entry.relative);
        if matches(&to, entry)? {
            continue;
        }
        prepare_parent(to.parent().context("Missing parent")?)?;
        let temporary = to.with_extension(format!("{}.tmp", super::revision()));
        fs::copy(&entry.path, &temporary)?;
        set_mode(&temporary, entry.executable)?;
        fs::rename(&temporary, &to)?;
    }
    Ok(())
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

    fn prepare(parent: &Path) -> anyhow::Result<()> {
        Ok(fs::create_dir_all(parent)?)
    }

    fn kept_package(store: &Path) -> (tempfile::TempDir, PackageRef) {
        let folder = tempfile::tempdir().unwrap();
        let entries = vec![
            entry(folder.path(), "bin/tool", &[0xff, 0x00, 0x80], true),
            entry(folder.path(), "notes/a.txt", b"hello", false),
        ];
        let package = describe(&entries).unwrap();
        keep(&entries, &package, store).unwrap();
        (folder, package)
    }

    #[test]
    fn a_kept_package_comes_back_byte_for_byte_with_its_mode() {
        let store = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let (folder, package) = kept_package(store.path());
        assert_eq!((package.files, package.bytes), (2, 8));
        // Keeping it again changes nothing, and leaves no temporary folder.
        let entries = verified(store.path(), &package).unwrap().unwrap();
        keep(&entries, &package, store.path()).unwrap();
        assert_eq!(fs::read_dir(store.path()).unwrap().count(), 1);
        drop(folder);
        let target = out.path().join("plugin");
        copy_to(&entries, &target, &prepare).unwrap();
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
    }

    #[test]
    fn whatever_is_in_the_target_it_ends_up_exactly_the_package() {
        let store = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let (_folder, package) = kept_package(store.path());
        let entries = verified(store.path(), &package).unwrap().unwrap();
        let target = out.path().join("plugin");
        copy_to(&entries, &target, &prepare).unwrap();
        // A changed file, a marker claiming it was copied, a planted file and a folder where a
        // file belongs.
        fs::write(target.join("bin/tool"), "tampered").unwrap();
        fs::write(target.join(".themis-package"), &package.hash).unwrap();
        fs::write(target.join("planted"), "x").unwrap();
        fs::remove_file(target.join("notes/a.txt")).unwrap();
        fs::create_dir(target.join("notes/a.txt")).unwrap();
        fs::write(target.join("notes/a.txt/inner"), "x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let elsewhere = tempfile::tempdir().unwrap();
            fs::write(elsewhere.path().join("kept"), "x").unwrap();
            std::os::unix::fs::symlink(elsewhere.path(), target.join("bin/link")).unwrap();
            let wanted = names(&entries);
            remove_others(&target, &wanted).unwrap();
            // A link is removed as a link: what it pointed at is untouched.
            assert!(elsewhere.path().join("kept").exists());
            fs::set_permissions(target.join("bin"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        copy_to(&entries, &target, &prepare).unwrap();
        assert_eq!(
            fs::read(target.join("bin/tool")).unwrap(),
            [0xff, 0x00, 0x80]
        );
        assert_eq!(fs::read(target.join("notes/a.txt")).unwrap(), b"hello");
        assert!(!target.join("planted").exists() && !target.join(".themis-package").exists());
        assert!(fs::symlink_metadata(target.join("bin/link")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_target_that_is_a_link_is_refused_and_a_changed_mode_is_put_back() {
        use std::os::unix::fs::PermissionsExt;
        let store = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let (_folder, package) = kept_package(store.path());
        let entries = verified(store.path(), &package).unwrap().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let link = out.path().join("link");
        std::os::unix::fs::symlink(elsewhere.path(), &link).unwrap();
        assert!(copy_to(&entries, &link, &prepare).is_err());
        assert_eq!(fs::read_dir(elsewhere.path()).unwrap().count(), 0);
        let target = out.path().join("plugin");
        copy_to(&entries, &target, &prepare).unwrap();
        let tool = target.join("bin/tool");
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o644)).unwrap();
        copy_to(&entries, &target, &prepare).unwrap();
        assert_eq!(
            fs::metadata(&tool).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    /// A project's store is in the project: it can be seeded with other content under the name
    /// of the right hash.
    #[test]
    fn a_stored_package_that_is_not_what_its_name_says_is_not_used_or_kept() {
        let store = tempfile::tempdir().unwrap();
        let (folder, package) = kept_package(store.path());
        let stored = store.path().join(&package.hash);
        fs::write(stored.join("bin/tool"), "tampered").unwrap();
        assert!(verified(store.path(), &package).unwrap().is_none());
        // Keeping the real files replaces it.
        let real = vec![
            entry(folder.path(), "bin/tool", &[0xff, 0x00, 0x80], true),
            entry(folder.path(), "notes/a.txt", b"hello", false),
        ];
        keep(&real, &package, store.path()).unwrap();
        assert_eq!(
            fs::read(stored.join("bin/tool")).unwrap(),
            [0xff, 0x00, 0x80]
        );
        assert!(verified(store.path(), &package).unwrap().is_some());
        // So are extra files and links.
        fs::write(stored.join("planted"), "x").unwrap();
        assert!(verified(store.path(), &package).unwrap().is_none());
        fs::remove_file(stored.join("planted")).unwrap();
        #[cfg(unix)]
        {
            let hash = package.hash.clone();
            fs::remove_dir_all(&stored).unwrap();
            let elsewhere = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(elsewhere.path(), store.path().join(hash)).unwrap();
            assert!(verified(store.path(), &package).unwrap().is_none());
        }
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
        let gone = PackageRef {
            hash: "a".repeat(64),
            files: 1,
            bytes: 1,
        };
        assert!(verified(store.path(), &gone).unwrap().is_none());
        for bad in ["", "../../etc", &"g".repeat(64), &"a".repeat(63)] {
            let package = PackageRef {
                hash: (*bad).to_owned(),
                files: 0,
                bytes: 0,
            };
            assert!(package.validate().is_err(), "{bad}");
            assert!(verified(store.path(), &package).is_err());
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
