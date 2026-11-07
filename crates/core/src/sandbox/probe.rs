//! Backend availability: probing each backend once per process and choosing a run's enforcement.

use super::*;

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
    pub(super) fn yes(detail: impl Into<String>, abi: u32) -> Self {
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

pub(super) fn choose_with(
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
