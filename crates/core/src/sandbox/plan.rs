//! Building a run's sandbox plan, its read-only and subagent variants, and wrapping commands under it.

use super::*;

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

    pub(super) fn with_enforcement(
        inputs: PlanInputs<'_>,
        enforcement: Enforcement,
        reason: String,
    ) -> Self {
        let home = inputs.home.map(Path::to_path_buf).or_else(home_dir);
        let mut writable = Vec::new();
        if inputs.open.is_some() {
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
        if let Some(home) = home.as_ref().filter(|_| inputs.open.is_none()) {
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
            open: inputs.open.is_some(),
            network: match inputs.open {
                Some(written) if !written.network_written => NetworkPolicy::Allow,
                _ => inputs.config.network,
            },
            network_enforced,
            allow_keychain: match inputs.open {
                Some(written) if !written.keychain_written => true,
                _ => inputs.config.allow_keychain,
            },
            landlock_abi,
            configured_writable: inputs.config.writable_paths.clone(),
            home,
            writable,
            protected,
            has_app_data: inputs.app_data.is_some(),
            secrets,
            read_only: false,
            #[cfg(target_os = "linux")]
            landlock: OnceLock::new(),
        }
    }

    /// The same plan for a read-only subagent: only the temporary folders stay writable, so
    /// commands can read the project and run tools that write scratch files, but cannot
    /// change the project. Network, secrets and protected folders are unchanged.
    pub fn read_only(&self) -> Self {
        let mut writable = Vec::new();
        for dir in temp_dirs() {
            push_unique(&mut writable, dir);
        }
        Self {
            enforcement: self.enforcement.clone(),
            reason: self.reason.clone(),
            open: self.open,
            network: self.network,
            network_enforced: self.network_enforced,
            allow_keychain: self.allow_keychain,
            landlock_abi: self.landlock_abi,
            configured_writable: Vec::new(),
            home: self.home.clone(),
            writable,
            protected: self.protected.clone(),
            has_app_data: self.has_app_data,
            secrets: self.secrets.clone(),
            read_only: true,
            #[cfg(target_os = "linux")]
            landlock: OnceLock::new(),
        }
    }

    /// A subagent's plan: read-only except for its own `paths`. Temporary folders stay writable
    /// unless the project lives in one, and YOLO changes nothing here: this wall is the
    /// harness's, not a default.
    pub fn confined_to(&self, root: &Path, paths: &[PathBuf]) -> Self {
        let mut plan = self.read_only();
        let root = resolve_existing(root);
        plan.writable.retain(|folder| !root.starts_with(folder));
        for path in paths {
            push_unique(&mut plan.writable, resolve_existing(path));
        }
        plan
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
            open: None,
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

    /// Why the sandbox blocked a step of a command that still exited successfully (a chain
    /// whose last command passed), or `None`. Only path-specific signals count, as in
    /// [`Self::denial`]; such a command is not escalated, the agent is told instead.
    pub fn blocked_in_success(&self, success: bool, output: &str, cwd: &Path) -> Option<String> {
        if !success || !self.is_active() {
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
            allow_keychain: self.allow_keychain,
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
            // A subagent's write paths are a harness wall: they hold with no OS sandbox too.
            return self.read_only.then(|| FileGuard {
                no_write: self.protected.clone(),
                no_read: Vec::new(),
                only_write: Some(self.writable.clone()),
            });
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
            only_write: self.read_only.then(|| self.writable.clone()),
        })
    }
}
