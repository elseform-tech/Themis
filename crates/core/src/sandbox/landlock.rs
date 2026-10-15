//! Linux Landlock ruleset applied by the child right before `exec`.
//!
//! The ruleset is built in the parent (opening paths and allocating is not
//! safe between `fork` and `exec`); the child only calls
//! `prctl(PR_SET_NO_NEW_PRIVS)` and `landlock_restrict_self`, both plain
//! system calls. Landlock only allows: reading is granted on `/` split around
//! the secret locations, writing on each writable folder split around the
//! protected folders and secrets (see [`super::split_around`]). The project's
//! own `.themis/` folder is not carved out of the project root, because that
//! would also forbid creating new files at the top of the project; the
//! app-data folder is.

use super::{split_around, NetworkPolicy, SandboxPlan};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

const CREATE_RULESET_VERSION: libc::c_uint = 1;
const RULE_PATH_BENEATH: libc::c_int = 1;

const EXECUTE: u64 = 1 << 0;
const WRITE_FILE: u64 = 1 << 1;
const READ_FILE: u64 = 1 << 2;
const READ_DIR: u64 = 1 << 3;
const REFER: u64 = 1 << 13;
const TRUNCATE: u64 = 1 << 14;
const IOCTL_DEV: u64 = 1 << 15;
/// Rights in ABI 1: bits 0 (execute) through 12 (make symlink).
const ABI1_FS: u64 = (1 << 13) - 1;
const READ: u64 = EXECUTE | READ_FILE | READ_DIR;
/// Rights that apply to a regular file (others are only valid on folders).
const FILE_RIGHTS: u64 = EXECUTE | WRITE_FILE | READ_FILE | TRUNCATE | IOCTL_DEV;
const NET_BIND_TCP: u64 = 1 << 0;
const NET_CONNECT_TCP: u64 = 1 << 1;
/// Directory entries visited while splitting grants around excluded paths.
const SPLIT_BUDGET: usize = 50_000;

#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
}

#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

/// The kernel's Landlock ABI version, or `None` when Landlock is unavailable.
pub fn abi() -> Option<u32> {
    // SAFETY: querying the version passes a null attribute with size 0.
    let version = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<RulesetAttr>(),
            0usize,
            CREATE_RULESET_VERSION,
        )
    };
    u32::try_from(version).ok().filter(|version| *version >= 1)
}

fn handled_fs(abi: u32) -> u64 {
    match abi {
        0 | 1 => ABI1_FS,
        2 => ABI1_FS | REFER,
        3 | 4 => ABI1_FS | REFER | TRUNCATE,
        _ => ABI1_FS | REFER | TRUNCATE | IOCTL_DEV,
    }
}

/// A built ruleset, shared by every command of a run.
#[derive(Debug)]
pub struct Ruleset {
    fd: OwnedFd,
}

impl Ruleset {
    /// Builds the ruleset for `plan` on a kernel with Landlock `abi`.
    pub fn build(plan: &SandboxPlan, abi: u32) -> Result<Self, String> {
        let fs = handled_fs(abi);
        let net = if plan.network() == NetworkPolicy::Block && abi >= 4 {
            NET_BIND_TCP | NET_CONNECT_TCP
        } else {
            0
        };
        let attr = RulesetAttr {
            handled_access_fs: fs,
            handled_access_net: net,
        };
        let size = if abi >= 4 {
            std::mem::size_of::<RulesetAttr>()
        } else {
            std::mem::size_of::<u64>()
        };
        // SAFETY: `attr` outlives the call and `size` never exceeds it.
        let raw = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &attr as *const RulesetAttr,
                size,
                0 as libc::c_uint,
            )
        };
        if raw < 0 {
            return Err(format!(
                "cannot create a Landlock ruleset: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: the kernel returned a new descriptor that we now own.
        let ruleset = Self {
            fd: unsafe { OwnedFd::from_raw_fd(raw as i32) },
        };
        let secrets = canonical(plan.secrets());
        let mut budget = SPLIT_BUDGET;
        let (readable, listable) = split_around(Path::new("/"), &secrets, &mut budget)?;
        for path in &readable {
            ruleset.allow(path, READ & fs)?;
        }
        for path in &listable {
            ruleset.allow(path, READ_DIR)?;
        }
        let write = fs & !READ;
        let mut excluded = canonical(plan.protected());
        // The project's own `.themis/` (last protected entry) cannot be carved
        // out of the project root without forbidding new top-level files.
        if let (Some(project_config), Some(root)) =
            (excluded.last().cloned(), plan.writable().first())
        {
            if project_config.parent() == Some(root.as_path()) {
                excluded.pop();
            }
        }
        excluded.extend(secrets.iter().cloned());
        for root in plan.writable() {
            let Ok(root) = root.canonicalize() else {
                continue;
            };
            let (writable, _) = split_around(&root, &excluded, &mut budget)?;
            for path in &writable {
                ruleset.allow(path, write | READ & fs)?;
            }
        }
        for device in [
            "/dev/null",
            "/dev/zero",
            "/dev/full",
            "/dev/tty",
            "/dev/ptmx",
        ] {
            let _ = ruleset.allow(Path::new(device), (WRITE_FILE | READ_FILE | IOCTL_DEV) & fs);
        }
        let _ = ruleset.allow(Path::new("/dev/pts"), (write | READ) & fs);
        Ok(ruleset)
    }

    /// Adds one path-beneath rule; files get only file rights.
    fn allow(&self, path: &Path, access: u64) -> Result<(), String> {
        let Ok(text) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return Ok(());
        };
        // SAFETY: `text` is a valid C string; O_PATH opens no file contents.
        let fd = unsafe { libc::open(text.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd < 0 {
            // Vanished or unreadable entries are simply not granted.
            return Ok(());
        }
        // SAFETY: `open` returned a new descriptor that we now own.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let is_dir = std::fs::metadata(path).is_ok_and(|meta| meta.is_dir());
        let allowed = if is_dir { access } else { access & FILE_RIGHTS };
        if allowed == 0 {
            return Ok(());
        }
        let attr = PathBeneathAttr {
            allowed_access: allowed,
            parent_fd: fd.as_raw_fd(),
        };
        // SAFETY: `attr` is a valid, packed rule that outlives the call.
        let result = unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                self.fd.as_raw_fd(),
                RULE_PATH_BENEATH,
                &attr as *const PathBeneathAttr,
                0 as libc::c_uint,
            )
        };
        if result < 0 {
            return Err(format!(
                "cannot add a Landlock rule for {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    /// Makes the child restrict itself to this ruleset right before `exec`.
    pub fn apply_on_exec(self: &std::sync::Arc<Self>, command: &mut std::process::Command) {
        let fd = self.fd.as_raw_fd();
        // SAFETY: the closure only performs async-signal-safe system calls and
        // allocates nothing; `fd` stays open in the parent until spawn returns
        // because the caller keeps the ruleset alive.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::syscall(libc::SYS_landlock_restrict_self, fd, 0 as libc::c_uint) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

fn canonical(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for path in paths {
        if let Ok(path) = path.canonicalize() {
            if !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}
