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
mod seatbelt;
mod split;

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
// Backend availability
// ---------------------------------------------------------------------------

/// Whether a backend can be used on this machine, with the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Availability {
    pub available: bool,
    pub detail: String,
    /// Landlock ABI version (0 when unknown or not Landlock).
    #[serde(skip)]
    pub abi: u32,
}

impl Availability {
    /// An unusable backend with the reason.
    pub fn no(detail: impl Into<String>) -> Self {
        Self {
            available: false,
            detail: detail.into(),
            abi: 0,
        }
    }
    fn yes(detail: impl Into<String>, abi: u32) -> Self {
        Self {
            available: true,
            detail: detail.into(),
            abi,
        }
    }
}

/// Probes `backend` once per process and caches the answer.
pub fn availability(backend: Backend) -> Availability {
    static CACHE: [OnceLock<Availability>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let index = Backend::ALL
        .iter()
        .position(|candidate| *candidate == backend)
        .unwrap_or(0);
    CACHE[index]
        .get_or_init(|| {
            if !backend.supported_here() {
                return Availability::no(format!("not available on {}", os_name()));
            }
            match backend {
                Backend::Seatbelt => probe_seatbelt(),
                Backend::Bubblewrap => probe_bubblewrap(),
                Backend::Landlock => probe_landlock(),
            }
        })
        .clone()
}

/// Runs a probe command with a short deadline; `true` on a zero exit.
fn probe_succeeds(program: &Path, args: &[&str]) -> Result<(), String> {
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start {}: {error}", program.display()))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    use std::io::Read;
                    let _ = (&mut pipe).take(512).read_to_string(&mut stderr);
                }
                let first = stderr.lines().next().unwrap_or("").trim();
                return Err(if first.is_empty() {
                    format!("{} exited with {status}", program.display())
                } else {
                    format!("{} failed: {first}", program.display())
                });
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{} did not finish", program.display()));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn probe_seatbelt() -> Availability {
    let program = Path::new(seatbelt::SANDBOX_EXEC);
    if !program.exists() {
        return Availability::no(format!("{} is missing", seatbelt::SANDBOX_EXEC));
    }
    let truth = if Path::new("/usr/bin/true").exists() {
        "/usr/bin/true"
    } else {
        "/bin/true"
    };
    match probe_succeeds(program, &["-p", "(version 1)(allow default)", truth]) {
        Ok(()) => Availability::yes(format!("{} works", seatbelt::SANDBOX_EXEC), 0),
        Err(error) => Availability::no(error),
    }
}

fn probe_bubblewrap() -> Availability {
    let Some(bwrap) = find_program("bwrap", std::env::var_os("PATH").as_deref()) else {
        return Availability::no("bwrap is not installed");
    };
    let truth = ["/usr/bin/true", "/bin/true"]
        .into_iter()
        .find(|path| Path::new(path).exists())
        .unwrap_or("true");
    match probe_succeeds(&bwrap, &["--ro-bind", "/", "/", "--dev", "/dev", truth]) {
        Ok(()) => Availability::yes(format!("{} works", bwrap.display()), 0),
        Err(error) => Availability::no(format!(
            "bwrap is installed but cannot create a sandbox here ({error}); user namespaces may be disabled"
        )),
    }
}

#[cfg(target_os = "linux")]
fn probe_landlock() -> Availability {
    match landlock::abi() {
        Some(abi) if abi >= 1 => Availability::yes(format!("Landlock ABI {abi}"), abi),
        _ => Availability::no(
            "Landlock is not enabled in this kernel (needs Linux 5.13 or later with Landlock on)",
        ),
    }
}

#[cfg(not(target_os = "linux"))]
fn probe_landlock() -> Availability {
    Availability::no(format!("not available on {}", os_name()))
}

/// Resolves `program` like `execvp` would, against the `PATH` value given.
pub fn find_program(program: &str, path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    if program.is_empty() {
        return None;
    }
    if program.contains('/') || (cfg!(windows) && program.contains('\\')) {
        let candidate = PathBuf::from(program);
        return candidate.is_file().then_some(candidate);
    }
    std::env::split_paths(path?)
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

// ---------------------------------------------------------------------------
// Resolution and plan
// ---------------------------------------------------------------------------

/// How commands are confined for one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enforcement {
    /// `sandbox.enabled` is false: commands run unsandboxed.
    Disabled,
    /// `auto` found no usable backend: commands run unsandboxed.
    Unavailable,
    /// Commands run inside this backend.
    Active(Backend),
    /// An explicitly configured backend is unusable: commands do not run.
    FailClosed(Backend, String),
}

/// Picks the enforcement for `config` on this machine, with a short reason.
pub fn choose(config: &SandboxConfig) -> (Enforcement, String) {
    choose_with(config, availability)
}

fn choose_with(
    config: &SandboxConfig,
    probe: impl Fn(Backend) -> Availability,
) -> (Enforcement, String) {
    if !config.enabled {
        return (
            Enforcement::Disabled,
            "turned off with sandbox.enabled: false".into(),
        );
    }
    if let Some(backend) = config.backend.backend() {
        let state = probe(backend);
        if !state.available {
            return (
                Enforcement::FailClosed(backend, state.detail.clone()),
                format!(
                    "{} is set in configuration but cannot be used: {}",
                    backend.label(),
                    state.detail
                ),
            );
        }
        if backend == Backend::Landlock && config.network == NetworkPolicy::Block && state.abi < 4 {
            return (
                Enforcement::FailClosed(
                    backend,
                    format!("network blocking needs Landlock ABI 4 (Linux 6.7); this kernel has ABI {}", state.abi),
                ),
                format!(
                    "Landlock is set in configuration with network: \"block\", but this kernel (ABI {}) cannot block network access",
                    state.abi
                ),
            );
        }
        return (Enforcement::Active(backend), "set in configuration".into());
    }
    let candidates: &[Backend] = if cfg!(target_os = "macos") {
        &[Backend::Seatbelt]
    } else if cfg!(target_os = "linux") {
        &[Backend::Bubblewrap, Backend::Landlock]
    } else {
        &[]
    };
    let mut skipped = Vec::new();
    for backend in candidates {
        let state = probe(*backend);
        if state.available {
            let mut reason = format!("chosen automatically for {}", os_name());
            if !skipped.is_empty() {
                reason.push_str(&format!(" ({})", skipped.join("; ")));
            }
            return (Enforcement::Active(*backend), reason);
        }
        skipped.push(format!("{}: {}", backend.label(), state.detail));
    }
    let mut reason = format!("not available on this system ({})", os_name());
    if !skipped.is_empty() {
        reason.push_str(&format!(": {}", skipped.join("; ")));
    }
    (Enforcement::Unavailable, reason)
}

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
    /// the protected folders stay read-only.
    pub open: bool,
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

/// The user's home folder from the environment.
pub fn home_dir() -> Option<PathBuf> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// Expands a validated configured path (`~/x` or absolute).
pub fn expand_path(path: &str, home: Option<&Path>) -> Option<PathBuf> {
    if path == "~" {
        return home.map(Path::to_path_buf);
    }
    match path.strip_prefix("~/") {
        Some(rest) => home.map(|home| home.join(rest)),
        None => Some(PathBuf::from(path)),
    }
}

/// Canonical form of `path`: the nearest existing ancestor is canonicalized
/// (symlinks resolved, `/tmp` -> `/private/tmp` on macOS) and the missing
/// remainder appended. `.`/`..` are resolved lexically first.
pub fn resolve_existing(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir => {}
            other => normal.push(other.as_os_str()),
        }
    }
    let mut ancestor = normal.as_path();
    let mut tail = Vec::new();
    loop {
        if let Ok(canonical) = ancestor.canonicalize() {
            let mut resolved = canonical;
            for part in tail.iter().rev() {
                resolved.push(part);
            }
            return resolved;
        }
        match (ancestor.file_name(), ancestor.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name.to_owned());
                ancestor = parent;
            }
            _ => return normal,
        }
    }
}

fn push_unique(list: &mut Vec<PathBuf>, path: PathBuf) {
    if !list.contains(&path) {
        list.push(path);
    }
}

/// Temporary folders sandboxed commands may write to.
fn temp_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let temp = resolve_existing(&std::env::temp_dir());
    // macOS keeps a per-user cache folder (`.../C`) beside the temp folder
    // (`.../T`); compilers and tools use it like a temp folder.
    if cfg!(target_os = "macos") && temp.file_name().is_some_and(|name| name == "T") {
        if let Some(cache) = temp.parent().map(|parent| parent.join("C")) {
            if cache.is_dir() {
                push_unique(&mut dirs, resolve_existing(&cache));
            }
        }
    }
    push_unique(&mut dirs, temp);
    if cfg!(unix) {
        for fixed in ["/tmp", "/var/tmp"] {
            if Path::new(fixed).is_dir() {
                push_unique(&mut dirs, resolve_existing(Path::new(fixed)));
            }
        }
    }
    if cfg!(target_os = "linux") && Path::new("/dev/shm").is_dir() {
        push_unique(&mut dirs, PathBuf::from("/dev/shm"));
    }
    dirs
}

impl SandboxPlan {
    /// Captures the sandbox for one run.
    pub fn new(inputs: PlanInputs<'_>) -> Self {
        let (enforcement, reason) = choose(inputs.config);
        Self::with_enforcement(inputs, enforcement, reason)
    }

    /// [`Self::new`] with backend availability decided by `probe` instead of
    /// probing this machine (tests use it to simulate a missing backend).
    pub fn with_availability(
        inputs: PlanInputs<'_>,
        probe: impl Fn(Backend) -> Availability,
    ) -> Self {
        let (enforcement, reason) = choose_with(inputs.config, probe);
        Self::with_enforcement(inputs, enforcement, reason)
    }

    fn with_enforcement(inputs: PlanInputs<'_>, enforcement: Enforcement, reason: String) -> Self {
        let home = inputs.home.map(Path::to_path_buf).or_else(home_dir);
        let mut writable = Vec::new();
        if inputs.open {
            // Everything; the protected folders below are carved back out.
            push_unique(&mut writable, PathBuf::from("/"));
        }
        push_unique(&mut writable, resolve_existing(inputs.project_root));
        for dir in temp_dirs() {
            push_unique(&mut writable, dir);
        }
        for path in &inputs.config.writable_paths {
            if let Some(path) = expand_path(path, home.as_deref()) {
                push_unique(&mut writable, resolve_existing(&path));
            }
        }
        let mut protected = Vec::new();
        if let Some(app_data) = inputs.app_data {
            push_unique(&mut protected, resolve_existing(app_data));
        }
        push_unique(
            &mut protected,
            resolve_existing(&inputs.project_root.join(".themis")),
        );
        let mut secrets = Vec::new();
        if let Some(home) = home.as_ref().filter(|_| !inputs.open) {
            for relative in SECRET_PATHS {
                if *relative == "Library/Keychains" && !cfg!(target_os = "macos") {
                    continue;
                }
                let path = home.join(relative);
                push_unique(&mut secrets, resolve_existing(&path));
                // Also the spelling without symlink resolution, for messages.
                push_unique(&mut secrets, path);
            }
        }
        let landlock_abi = match &enforcement {
            Enforcement::Active(Backend::Landlock) => availability(Backend::Landlock).abi,
            _ => 0,
        };
        let network_enforced = match &enforcement {
            Enforcement::Active(Backend::Landlock) => landlock_abi >= 4,
            Enforcement::Active(_) => true,
            _ => false,
        };
        Self {
            enforcement,
            reason,
            open: inputs.open,
            network: if inputs.open {
                NetworkPolicy::Allow
            } else {
                inputs.config.network
            },
            network_enforced,
            allow_keychain: inputs.open || inputs.config.allow_keychain,
            landlock_abi,
            configured_writable: inputs.config.writable_paths.clone(),
            home,
            writable,
            protected,
            has_app_data: inputs.app_data.is_some(),
            secrets,
            #[cfg(target_os = "linux")]
            landlock: OnceLock::new(),
        }
    }

    /// A plan that never sandboxes (the sandbox is off).
    pub fn disabled(project_root: &Path) -> Self {
        let config = SandboxConfig {
            enabled: false,
            ..SandboxConfig::default()
        };
        Self::new(PlanInputs {
            config: &config,
            project_root,
            app_data: None,
            home: None,
            open: false,
        })
    }

    pub fn enforcement(&self) -> &Enforcement {
        &self.enforcement
    }

    /// Whether this is YOLO's open plan.
    pub fn open(&self) -> bool {
        self.open
    }

    /// Whether commands run inside a backend.
    pub fn is_active(&self) -> bool {
        matches!(self.enforcement, Enforcement::Active(_))
    }

    /// Whether the sandbox is switched on (even when no backend is usable).
    pub fn enabled(&self) -> bool {
        self.enforcement != Enforcement::Disabled
    }

    /// The error every command gets when the configured backend is unusable.
    pub fn refusal(&self) -> Option<String> {
        match &self.enforcement {
            Enforcement::FailClosed(backend, detail) => Some(format!(
                "the sandbox backend \"{}\" set in runtime.jsonc cannot be used on this machine ({detail}), so commands do not run. Install or enable {}, or set sandbox.backend to \"auto\" in your user runtime.jsonc",
                backend.id(),
                backend.label()
            )),
            _ => None,
        }
    }

    /// Whether sandboxed commands may use the macOS Keychain.
    pub fn allow_keychain(&self) -> bool {
        self.allow_keychain
    }

    pub fn network(&self) -> NetworkPolicy {
        self.network
    }

    /// Folders sandboxed commands may write to (canonical).
    pub fn writable(&self) -> &[PathBuf] {
        &self.writable
    }

    /// Folders sandboxed commands can never write to (canonical).
    pub fn protected(&self) -> &[PathBuf] {
        &self.protected
    }

    /// Secret locations sandboxed commands can neither read nor write.
    pub fn secrets(&self) -> &[PathBuf] {
        &self.secrets
    }

    /// Wraps `program args` for this plan. `path` is the child's `PATH`, used to
    /// resolve the program before it is handed to a wrapper.
    pub fn prepare(
        &self,
        program: &str,
        args: &[String],
        path: Option<&std::ffi::OsStr>,
    ) -> std::io::Result<Prepared> {
        let backend = match &self.enforcement {
            Enforcement::Active(backend) => *backend,
            Enforcement::FailClosed(..) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    self.refusal().unwrap_or_default(),
                ))
            }
            Enforcement::Disabled | Enforcement::Unavailable => {
                return Ok(Prepared::plain(program, args));
            }
        };
        match backend {
            Backend::Seatbelt => {
                let resolved = find_program(program, path).ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "No such file or directory (os error 2)",
                    )
                })?;
                let (profile, params) = seatbelt::profile(self);
                Ok(Prepared {
                    program: seatbelt::SANDBOX_EXEC.into(),
                    args: seatbelt::arguments(&profile, &params, &resolved, args),
                    #[cfg(target_os = "linux")]
                    landlock: None,
                })
            }
            Backend::Bubblewrap => {
                let resolved = find_program(program, path).ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "No such file or directory (os error 2)",
                    )
                })?;
                let bwrap = find_program("bwrap", std::env::var_os("PATH").as_deref())
                    .unwrap_or_else(|| PathBuf::from("bwrap"));
                let mut arguments = bubblewrap::arguments(self);
                arguments.push("--".into());
                arguments.push(resolved.into_os_string());
                arguments.extend(args.iter().map(Into::into));
                Ok(Prepared {
                    program: bwrap.into_os_string(),
                    args: arguments,
                    #[cfg(target_os = "linux")]
                    landlock: None,
                })
            }
            Backend::Landlock => {
                #[cfg(target_os = "linux")]
                {
                    let ruleset = self
                        .landlock
                        .get_or_init(|| {
                            landlock::Ruleset::build(self, self.landlock_abi).map(Arc::new)
                        })
                        .clone()
                        .map_err(|error| {
                            std::io::Error::new(std::io::ErrorKind::PermissionDenied, error)
                        })?;
                    let mut prepared = Prepared::plain(program, args);
                    prepared.landlock = Some(ruleset);
                    Ok(prepared)
                }
                #[cfg(not(target_os = "linux"))]
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "Landlock is only available on Linux",
                ))
            }
        }
    }

    /// Why a failed sandboxed command was most likely blocked by the sandbox,
    /// or `None` when nothing points at the sandbox. See `docs/sandbox.md` for
    /// the signals and their limits.
    pub fn denial(&self, success: bool, output: &str, cwd: &Path) -> Option<String> {
        if success || !self.is_active() {
            return None;
        }
        detect::denial(self, output, cwd)
    }

    /// Whether a failed command's output mentions a permission or network
    /// error at all, so a short note about the sandbox helps the agent.
    pub fn may_have_blocked(&self, success: bool, output: &str) -> bool {
        !success && self.is_active() && detect::has_marker(self, output)
    }

    /// `program args` with no sandbox at all.
    pub fn unsandboxed(program: &str, args: &[String]) -> std::io::Result<Prepared> {
        Ok(Prepared::plain(program, args))
    }

    /// A read-only summary of this plan and the machine's backends.
    pub fn status(&self) -> SandboxStatus {
        let (state, backend) = match &self.enforcement {
            Enforcement::Disabled => ("off", None),
            Enforcement::Unavailable => ("unavailable", None),
            Enforcement::Active(backend) => ("on", Some(*backend)),
            Enforcement::FailClosed(backend, _) => ("blocked", Some(*backend)),
        };
        let summary = match (&self.enforcement, backend) {
            (Enforcement::Disabled, _) => "Off".to_owned(),
            (Enforcement::Unavailable, _) => {
                format!("Not available on this system ({})", os_name())
            }
            (Enforcement::Active(_), Some(backend)) => {
                format!("{}, {}", backend.label(), self.reason)
            }
            (_, Some(backend)) => format!(
                "{} is configured but cannot be used here, so commands do not run",
                backend.label()
            ),
            _ => String::new(),
        };
        let network_note = match (&self.enforcement, self.network) {
            (Enforcement::Active(_), NetworkPolicy::Block) if !self.network_enforced => Some(format!(
                "not enforced: Landlock ABI {} cannot block network access (needs ABI 4, Linux 6.7)",
                self.landlock_abi
            )),
            (Enforcement::Active(Backend::Landlock), NetworkPolicy::Block) => {
                Some("TCP only; Landlock does not restrict UDP".to_owned())
            }
            _ => None,
        };
        SandboxStatus {
            enabled: self.enabled(),
            state,
            backend: backend.map(Backend::label),
            summary,
            reason: self.reason.clone(),
            network: self.network,
            network_enforced: self.network_enforced,
            network_note,
            writable_paths: self
                .writable
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            configured_writable_paths: self.configured_writable.clone(),
            protected_paths: self
                .protected
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            // YOLO reads them too.
            secret_paths: SECRET_PATHS
                .iter()
                .filter(|_| !self.open)
                .filter(|path| **path != "Library/Keychains" || cfg!(target_os = "macos"))
                .map(|path| format!("~/{path}"))
                .collect(),
            open: self.open,
            backends: Backend::ALL
                .iter()
                .map(|backend| {
                    let state = availability(*backend);
                    BackendInfo {
                        id: backend.id(),
                        label: backend.label(),
                        os: backend.os(),
                        supported_here: backend.supported_here(),
                        available: state.available,
                        detail: if backend.supported_here() {
                            state.detail
                        } else {
                            format!("not available on {}", os_name())
                        },
                    }
                })
                .collect(),
            covers: COVERED.to_vec(),
            not_covered: NOT_COVERED.to_vec(),
        }
    }

    /// Guard for Themis' in-process file tools under this plan: the app-data
    /// folder is never writable, secrets are never readable, and with `yolo`
    /// (unrestricted paths) the project's `.themis/` folder is not writable
    /// either. `None` when the sandbox is off.
    pub fn file_guard(&self, yolo: bool) -> Option<FileGuard> {
        if !self.enabled() {
            return None;
        }
        let mut no_write = Vec::new();
        if self.has_app_data {
            no_write.extend(self.protected.first().cloned());
        }
        if yolo {
            no_write.extend(self.protected.last().cloned());
        }
        Some(FileGuard {
            no_write,
            no_read: self.secrets.clone(),
        })
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
