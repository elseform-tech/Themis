//! Operating-system sandbox for the commands an agent runs.
//!
//! Covered: the `shell` tool (blocking calls and background tasks), the `git`
//! tool and automation watch checks. Not covered: command-based MCP servers and
//! plugin hooks (long-lived integrations a person enabled; see
//! `docs/sandbox.md`), and Themis' own in-process work.
//!
//! A sandboxed command may write only to the run's project root, temporary
//! folders and the configured `sandbox.writable_paths`. It may read everything
//! except well-known secret locations ([`SECRET_PATHS`]), and it can never
//! write to the Themis app-data folder (configuration, chat grants, MCP consent,
//! plugin registry, sessions) or to the project's `.themis/` folder, so an agent
//! cannot grant itself permissions. Network access follows `sandbox.network`.
//!
//! In YOLO the plan is *open*: commands may write anywhere, read secret
//! locations, use the network and the Keychain; only the app-data folder and the
//! project's `.themis/` stay read-only, so an agent still cannot grant itself
//! permissions. Landlock cannot carve `.themis/` out of a writable tree, so
//! there the file tools alone protect it.
//!
//! Backends: Seatbelt (`/usr/bin/sandbox-exec`) on macOS; bubblewrap, else
//! Landlock, on Linux. Windows has none yet. `backend: "auto"` never blocks a
//! command: without a usable backend commands run unsandboxed and the status
//! says so. A backend named explicitly that is unusable fails closed.

use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
#[cfg(target_os = "linux")]
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

mod bubblewrap;
#[cfg(target_os = "linux")]
mod landlock;
mod paths;
mod plan;
mod probe;
mod seatbelt;
mod split;

pub use paths::{
    expand_path, home_dir, normalize_write_path, resolve_existing, write_paths_overlap,
};
use paths::{push_unique, temp_dirs};
use probe::choose_with;
pub use probe::{availability, choose, find_program, Availability, Enforcement};
pub use split::split_around;

/// Most `sandbox.writable_paths` entries accepted.
pub const MAX_WRITABLE_PATHS: usize = 32;
/// Longest accepted configured path, in bytes.
pub const MAX_PATH_BYTES: usize = 1024;

/// Secret locations relative to the home folder that sandboxed commands can
/// neither read nor write. `Library/Keychains` only matters on macOS.
pub const SECRET_PATHS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".config/gcloud",
    ".kube",
    ".netrc",
    ".git-credentials",
    ".config/gh",
    ".password-store",
    ".local/share/keyrings",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".pypirc",
    ".gem/credentials",
    ".vault-token",
    ".terraform.d/credentials.tfrc.json",
    "Library/Keychains",
];

/// Which backend to use; `auto` picks the best one available on this system.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackendChoice {
    #[default]
    Auto,
    Seatbelt,
    Bubblewrap,
    Landlock,
}

impl BackendChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Seatbelt => "seatbelt",
            Self::Bubblewrap => "bubblewrap",
            Self::Landlock => "landlock",
        }
    }

    fn backend(self) -> Option<Backend> {
        match self {
            Self::Auto => None,
            Self::Seatbelt => Some(Backend::Seatbelt),
            Self::Bubblewrap => Some(Backend::Bubblewrap),
            Self::Landlock => Some(Backend::Landlock),
        }
    }
}

/// Whether sandboxed commands may use the network.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    #[default]
    Allow,
    Block,
}

/// The `sandbox` block of `runtime.jsonc`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxConfig {
    pub enabled: bool,
    pub backend: BackendChoice,
    pub network: NetworkPolicy,
    /// Extra folders sandboxed commands may write to: absolute, or `~/...`.
    pub writable_paths: Vec<String>,
    /// macOS: let sandboxed commands use the Keychain (for example git's HTTPS credential
    /// helper). Off by default, so an agent's command can neither read saved passwords nor
    /// raise Keychain prompts.
    pub allow_keychain: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            backend: BackendChoice::Auto,
            network: NetworkPolicy::Allow,
            writable_paths: Vec::new(),
            allow_keychain: false,
        }
    }
}

/// A concrete sandbox implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Seatbelt,
    Bubblewrap,
    Landlock,
}

impl Backend {
    pub const ALL: [Backend; 3] = [Backend::Seatbelt, Backend::Bubblewrap, Backend::Landlock];

    pub fn id(self) -> &'static str {
        match self {
            Self::Seatbelt => "seatbelt",
            Self::Bubblewrap => "bubblewrap",
            Self::Landlock => "landlock",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Seatbelt => "Seatbelt",
            Self::Bubblewrap => "bubblewrap",
            Self::Landlock => "Landlock",
        }
    }

    /// Operating system the backend exists on.
    pub fn os(self) -> &'static str {
        match self {
            Self::Seatbelt => "macOS",
            Self::Bubblewrap | Self::Landlock => "Linux",
        }
    }

    /// Whether this operating system has the backend at all.
    pub fn supported_here(self) -> bool {
        match self {
            Self::Seatbelt => cfg!(target_os = "macos"),
            Self::Bubblewrap | Self::Landlock => cfg!(target_os = "linux"),
        }
    }
}

/// Name of this operating system, as shown in status text.
pub fn os_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else if cfg!(windows) {
        "Windows"
    } else {
        "this system"
    }
}

/// Validates a `sandbox` block on its own (shape, OS support, bounded paths).
pub fn validate(config: &SandboxConfig) -> Result<()> {
    if let Some(backend) = config.backend.backend() {
        ensure!(
            backend.supported_here(),
            "sandbox.backend \"{}\" is not supported on {} (it is a {} sandbox); use \"auto\"{}",
            backend.id(),
            os_name(),
            backend.os(),
            supported_names()
        );
    }
    ensure!(
        config.writable_paths.len() <= MAX_WRITABLE_PATHS,
        "sandbox.writable_paths allows at most {MAX_WRITABLE_PATHS} paths"
    );
    for path in &config.writable_paths {
        validate_writable_path(path)?;
    }
    Ok(())
}

fn supported_names() -> String {
    Backend::ALL
        .iter()
        .filter(|backend| backend.supported_here())
        .map(|backend| format!(" or \"{}\"", backend.id()))
        .collect()
}

/// A writable path is absolute or starts with `~/`, has no `..` component and
/// no control characters, and is at most [`MAX_PATH_BYTES`] long.
pub fn validate_writable_path(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && path.len() <= MAX_PATH_BYTES,
        "sandbox.writable_paths entries must be 1..{MAX_PATH_BYTES} bytes"
    );
    ensure!(
        !path.chars().any(char::is_control),
        "sandbox.writable_paths entries must not contain control characters"
    );
    let relative = path
        .strip_prefix("~/")
        .or(if path == "~" { Some("") } else { None });
    let absolute = relative.is_some() || path.starts_with('/') || Path::new(path).is_absolute();
    ensure!(
        absolute,
        "sandbox.writable_paths entries must be absolute or start with ~/ (got {path:?})"
    );
    ensure!(
        !path.split(['/', '\\']).any(|part| part == ".."),
        "sandbox.writable_paths entries must not contain .. (got {path:?})"
    );
    Ok(())
}

/// Checks that a project or run layer's `sandbox` object only tightens
/// `current`: it may enable the sandbox, block the network and drop writable
/// paths, but it may not disable the sandbox, change the backend, allow a
/// blocked network or add writable paths.
pub fn check_restriction(
    current: &SandboxConfig,
    layer: &serde_json::Value,
    origin: &str,
) -> Result<()> {
    let Some(map) = layer.as_object() else {
        bail!("{origin} sandbox configuration must be an object");
    };
    for (key, value) in map {
        match key.as_str() {
            "enabled" => {
                ensure!(
                    value.as_bool() != Some(false),
                    "{origin} configuration cannot disable the sandbox (sandbox.enabled); only your user runtime.jsonc can"
                );
            }
            "backend" => {
                let backend: BackendChoice = serde_json::from_value(value.clone())
                    .map_err(|e| anyhow::anyhow!("invalid sandbox.backend: {e}"))?;
                ensure!(
                    backend == current.backend,
                    "{origin} configuration cannot change sandbox.backend; only your user runtime.jsonc can"
                );
            }
            "network" => {
                let network: NetworkPolicy = serde_json::from_value(value.clone())
                    .map_err(|e| anyhow::anyhow!("invalid sandbox.network: {e}"))?;
                ensure!(
                    network == NetworkPolicy::Block || current.network == NetworkPolicy::Allow,
                    "{origin} configuration cannot allow network access that your user configuration blocks"
                );
            }
            "writable_paths" => {
                let paths: Vec<String> = serde_json::from_value(value.clone())
                    .map_err(|e| anyhow::anyhow!("invalid sandbox.writable_paths: {e}"))?;
                for path in paths {
                    ensure!(
                        current.writable_paths.contains(&path),
                        "{origin} configuration cannot add sandbox.writable_paths ({path:?}); it may only remove entries"
                    );
                }
            }
            "allow_keychain" => {
                ensure!(
                    value.as_bool() != Some(true) || current.allow_keychain,
                    "{origin} configuration cannot allow the Keychain (sandbox.allow_keychain); only your user runtime.jsonc can"
                );
            }
            // Unknown fields fail later with the ordinary schema error.
            _ => {}
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Resolution and plan
// ---------------------------------------------------------------------------

/// Inputs for [`SandboxPlan::new`].
#[derive(Debug, Clone)]
pub struct PlanInputs<'a> {
    pub config: &'a SandboxConfig,
    /// The run's project (or worktree) root.
    pub project_root: &'a Path,
    /// Themis app-data folder; never writable from the sandbox.
    pub app_data: Option<&'a Path>,
    /// Home folder used for `~/` paths and the secret list; `None` uses `$HOME`.
    pub home: Option<&'a Path>,
    /// YOLO: writable everywhere, no secret locations, network and Keychain on; only
    /// the protected folders stay read-only, plus the blocks a person wrote.
    pub open: Option<YoloSandbox>,
}

/// Sandbox settings a person (or a project, or a run override) wrote, which YOLO's
/// open plan keeps: it removes Themis' default walls, never theirs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct YoloSandbox {
    /// `network` was set, so a `"block"` there holds.
    pub network_written: bool,
    /// `allow_keychain` was set, so a `false` there holds.
    pub keychain_written: bool,
}

/// The sandbox a run's commands use, captured once per run.
pub struct SandboxPlan {
    enforcement: Enforcement,
    reason: String,
    /// YOLO's open plan (see [`PlanInputs::open`]).
    open: bool,
    network: NetworkPolicy,
    network_enforced: bool,
    allow_keychain: bool,
    landlock_abi: u32,
    configured_writable: Vec<String>,
    home: Option<PathBuf>,
    writable: Vec<PathBuf>,
    /// App data first (when known), then the project's `.themis/`.
    protected: Vec<PathBuf>,
    /// Whether `protected` starts with the app-data folder.
    has_app_data: bool,
    secrets: Vec<PathBuf>,
    /// A subagent's read-only copy: in-process file tools may write only `writable`.
    read_only: bool,
    #[cfg(target_os = "linux")]
    landlock: OnceLock<Result<Arc<landlock::Ruleset>, String>>,
}

impl std::fmt::Debug for SandboxPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxPlan")
            .field("enforcement", &self.enforcement)
            .field("network", &self.network)
            .field("writable", &self.writable)
            .finish_non_exhaustive()
    }
}

/// What the sandbox covers, for status views.
pub const COVERED: &[&str] = &[
    "shell commands",
    "background tasks",
    "git tool",
    "automation watch checks",
];
/// What the sandbox does not cover, for status views.
pub const NOT_COVERED: &[&str] = &["MCP servers", "plugin hooks"];

/// A command ready to spawn under a plan.
pub struct Prepared {
    pub program: std::ffi::OsString,
    pub args: Vec<std::ffi::OsString>,
    #[cfg(target_os = "linux")]
    landlock: Option<Arc<landlock::Ruleset>>,
}

impl Prepared {
    fn plain(program: &str, args: &[String]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(Into::into).collect(),
            #[cfg(target_os = "linux")]
            landlock: None,
        }
    }

    /// A `Command` for this program; on Landlock the child restricts itself
    /// right before `exec`. Keep `self` alive until the child has spawned.
    pub fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(&self.program);
        command.args(&self.args);
        #[cfg(target_os = "linux")]
        if let Some(ruleset) = &self.landlock {
            ruleset.apply_on_exec(&mut command);
        }
        command
    }
}

/// Read-only status of the sandbox for the permissions view.
#[derive(Debug, Clone, Serialize)]
pub struct SandboxStatus {
    pub enabled: bool,
    /// `on`, `off`, `unavailable` or `blocked` (configured backend unusable).
    pub state: &'static str,
    pub backend: Option<&'static str>,
    pub summary: String,
    pub reason: String,
    pub network: NetworkPolicy,
    pub network_enforced: bool,
    pub network_note: Option<String>,
    pub writable_paths: Vec<String>,
    pub configured_writable_paths: Vec<String>,
    pub protected_paths: Vec<String>,
    pub secret_paths: Vec<String>,
    /// YOLO's open plan: writable everywhere except the protected folders.
    pub open: bool,
    /// Whether commands may use the macOS Keychain under this plan.
    pub allow_keychain: bool,
    pub backends: Vec<BackendInfo>,
    pub covers: Vec<&'static str>,
    pub not_covered: Vec<&'static str>,
}

/// One backend's availability on this machine.
#[derive(Debug, Clone, Serialize)]
pub struct BackendInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub os: &'static str,
    pub supported_here: bool,
    pub available: bool,
    pub detail: String,
}

/// Path checks for Themis' in-process file tools (see [`SandboxPlan::file_guard`]).
#[derive(Debug, Clone, Default)]
pub struct FileGuard {
    no_write: Vec<PathBuf>,
    no_read: Vec<PathBuf>,
    /// Set for a read-only plan: writes outside these folders are refused.
    only_write: Option<Vec<PathBuf>>,
}

impl FileGuard {
    /// Refuses reading `path` when it is a protected secret location.
    pub fn check_read(&self, path: &Path) -> std::result::Result<(), String> {
        let resolved = resolve_existing(path);
        if self
            .no_read
            .iter()
            .any(|secret| resolved.starts_with(secret) || path.starts_with(secret))
        {
            return Err(format!(
                "'{}' is a protected secret location; the sandbox does not let agents read it",
                path.display()
            ));
        }
        Ok(())
    }

    /// Refuses writing `path` when it is protected (app data, project
    /// `.themis/` under YOLO) or a secret location.
    pub fn check_write(&self, path: &Path) -> std::result::Result<(), String> {
        self.check_read(path)?;
        let resolved = resolve_existing(path);
        if self
            .no_write
            .iter()
            .any(|protected| resolved.starts_with(protected) || path.starts_with(protected))
        {
            return Err(format!(
                "'{}' holds Themis settings or permissions; the sandbox does not let agents change it. Edit it yourself if needed",
                path.display()
            ));
        }
        if let Some(allowed) = &self.only_write {
            // A dangling link looks like a new file here, but a write would land at its target.
            let dangling = path.ancestors().any(|step| {
                step.symlink_metadata().is_ok_and(|meta| meta.is_symlink())
                    && step.canonicalize().is_err()
            });
            if dangling {
                return Err(format!(
                    "'{}' goes through a broken link; this subagent may not write through it",
                    path.display()
                ));
            }
            // Only the resolved path counts, and `..` is never followed, so a path cannot
            // start inside an allowed folder and step out of it.
            let climbs = path
                .components()
                .any(|part| part == std::path::Component::ParentDir);
            if climbs || !allowed.iter().any(|folder| resolved.starts_with(folder)) {
                return Err(format!(
                    "'{}' is outside the folders this subagent may write",
                    path.display()
                ));
            }
        }
        Ok(())
    }
}

mod detect;

/// Entry points for the `fuzz/` cargo-fuzz targets; not a stable API.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing {
    use std::ffi::OsString;
    use std::path::PathBuf;

    /// The Seatbelt profile and parameters for these path lists.
    pub fn seatbelt_profile(
        writable: &[PathBuf],
        protected: &[PathBuf],
        secrets: &[PathBuf],
        block_network: bool,
    ) -> (String, Vec<(String, OsString)>) {
        super::seatbelt::build(writable, protected, secrets, block_network)
    }

    /// Path-like words the denial detector reads from one output line.
    pub fn path_candidates(line: &str) -> Vec<String> {
        super::detect::path_candidates(line)
    }
}

#[cfg(test)]
mod tests;
