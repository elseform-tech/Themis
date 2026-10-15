//! Linux bubblewrap (`bwrap`) arguments.
//!
//! The whole filesystem is bound read-only, then the writable folders are
//! bound read-write on top, then the protected folders read-only again (later
//! mounts cover earlier ones), and secret folders are hidden behind an empty
//! read-only tmpfs (secret files behind `/dev/null`). `--unshare-net` blocks
//! the network. Each path is its own argument, so no path is ever parsed.

use super::{NetworkPolicy, SandboxPlan};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Arguments placed before `-- program args`.
pub fn arguments(plan: &SandboxPlan) -> Vec<OsString> {
    build(
        plan.writable(),
        plan.protected(),
        plan.secrets(),
        plan.network() == NetworkPolicy::Block,
    )
}

fn canonical_existing<P: AsRef<Path>>(paths: &[P]) -> Vec<(PathBuf, bool)> {
    let mut out: Vec<(PathBuf, bool)> = Vec::new();
    for path in paths {
        let Ok(canonical) = path.as_ref().canonicalize() else {
            continue;
        };
        let is_dir = canonical.is_dir();
        if !out.iter().any(|(seen, _)| *seen == canonical) {
            out.push((canonical, is_dir));
        }
    }
    out
}

/// Arguments for explicit path lists (see [`arguments`]).
pub fn build<P: AsRef<Path>>(
    writable: &[P],
    protected: &[P],
    secrets: &[P],
    block_network: bool,
) -> Vec<OsString> {
    let mut out: Vec<OsString> = ["--ro-bind", "/", "/", "--dev", "/dev"]
        .into_iter()
        .map(OsString::from)
        .collect();
    let pair = |out: &mut Vec<OsString>, flag: &str, source: &Path, target: &Path| {
        out.push(flag.into());
        out.push(source.as_os_str().to_owned());
        out.push(target.as_os_str().to_owned());
    };
    for (path, is_dir) in canonical_existing(writable) {
        if is_dir {
            pair(&mut out, "--bind", &path, &path);
        }
    }
    for (path, _) in canonical_existing(protected) {
        pair(&mut out, "--ro-bind", &path, &path);
    }
    for (path, is_dir) in canonical_existing(secrets) {
        if is_dir {
            out.push("--tmpfs".into());
            out.push(path.as_os_str().to_owned());
            out.push("--remount-ro".into());
            out.push(path.as_os_str().to_owned());
        } else {
            pair(&mut out, "--ro-bind", Path::new("/dev/null"), &path);
        }
    }
    if block_network {
        out.push("--unshare-net".into());
    }
    out.push("--die-with-parent".into());
    out
}
