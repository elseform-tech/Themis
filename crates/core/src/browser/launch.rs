//! Finding and starting a Chromium browser for the agent.
//!
//! The browser runs headless with its own profile directory (never the
//! person's real profile), a debugging port chosen by the OS, and no first-run
//! UI. Its process group is killed when the handle drops, and when Themis
//! itself dies without dropping it (a crash or force quit) by a guard process
//! on Unix and a kill-on-close job on Windows.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};

use super::cdp::Cdp;

/// How long a starting browser may take to report its debugging address.
const START_TIMEOUT: Duration = Duration::from_secs(20);
/// Initial window size; the Workspace view resizes the page to fit itself.
pub const DEFAULT_VIEWPORT: (u32, u32) = (1280, 800);

/// Usual install locations of Chromium browsers, most preferred first.
fn candidates() -> Vec<PathBuf> {
    let mut list: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        for app in [
            "Google Chrome.app/Contents/MacOS/Google Chrome",
            "Chromium.app/Contents/MacOS/Chromium",
            "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "Brave Browser.app/Contents/MacOS/Brave Browser",
            "Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
        ] {
            list.push(Path::new("/Applications").join(app));
            if let Some(home) = std::env::var_os("HOME") {
                list.push(Path::new(&home).join("Applications").join(app));
            }
        }
    } else if cfg!(windows) {
        for base in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(base) {
                let base = PathBuf::from(base);
                list.push(base.join("Google/Chrome/Application/chrome.exe"));
                list.push(base.join("Microsoft/Edge/Application/msedge.exe"));
                list.push(base.join("Chromium/Application/chrome.exe"));
            }
        }
    } else {
        for name in [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "microsoft-edge",
            "brave-browser",
        ] {
            if let Some(path) = std::env::var_os("PATH") {
                for dir in std::env::split_paths(&path) {
                    list.push(dir.join(name));
                }
            }
        }
    }
    list
}

/// The browser to use: the configured one when given (it must exist), a
/// downloaded private copy, else the first installed Chromium browser.
pub fn find_browser(configured: Option<&Path>, downloaded: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = configured {
        return path.is_file().then(|| path.to_path_buf());
    }
    downloaded
        .filter(|path| path.is_file())
        .map(Path::to_path_buf)
        .or_else(|| candidates().into_iter().find(|path| path.is_file()))
}

/// The arguments a browser is started with.
pub fn launch_args(profile: &Path, headless: bool) -> Vec<String> {
    let mut args = vec![
        format!("--user-data-dir={}", profile.display()),
        "--remote-debugging-port=0".to_owned(),
        "--no-first-run".to_owned(),
        "--no-default-browser-check".to_owned(),
        // Never touch the person's Keychain or saved passwords.
        "--use-mock-keychain".to_owned(),
        "--password-store=basic".to_owned(),
        // CalculateNativeWinOcclusion: Windows otherwise counts a headless
        // window as covered and stops painting its page.
        "--disable-features=Translate,MediaRouter,OptimizationHints,CalculateNativeWinOcclusion"
            .to_owned(),
        "--disable-background-timer-throttling".to_owned(),
        "--disable-renderer-backgrounding".to_owned(),
        "--disable-backgrounding-occluded-windows".to_owned(),
        // No --mute-audio: the Workspace speaker mutes pages (audio.js).
        // Headless screencasts ignore emulated device scale; render at Retina
        // scale so the Workspace view stays sharp (frames are capped to the view).
        "--force-device-scale-factor=2".to_owned(),
        format!(
            "--window-size={},{}",
            DEFAULT_VIEWPORT.0, DEFAULT_VIEWPORT.1
        ),
    ];
    if headless {
        args.push("--headless=new".to_owned());
    }
    // Chromium will not start as root with its sandbox (Linux containers and
    // CI images often run as root); it has no other way to run there.
    if running_as_root() {
        args.push("--no-sandbox".to_owned());
    }
    args.push("about:blank".to_owned());
    args
}

#[cfg(target_os = "linux")]
fn running_as_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(target_os = "linux"))]
fn running_as_root() -> bool {
    false
}

/// The `ws://` address from a "DevTools listening on ..." line.
pub fn devtools_address(line: &str) -> Option<String> {
    let start = line.find("ws://")?;
    let address = line[start..].split_whitespace().next()?;
    let rest = address.strip_prefix("ws://")?;
    let (host, path) = rest.split_once('/')?;
    let (name, port) = host.rsplit_once(':')?;
    (matches!(name, "127.0.0.1" | "localhost" | "[::1]")
        && port.parse::<u16>().is_ok()
        && path.starts_with("devtools/browser/"))
    .then(|| address.to_owned())
}

/// Run by the guard: `read` returns when the pipe from Themis closes, which
/// the OS does however Themis exits; the guard then ends the browser's group.
/// It ignores the TERM it sends itself and goes with the KILL. No `--` before
/// the group: dash (Debian and Ubuntu's `/bin/sh`) rejects it.
#[cfg(unix)]
const GUARD: &str =
    r#"read _; trap '' TERM; kill -TERM "-$1" 2>/dev/null; sleep 3; kill -KILL "-$1" 2>/dev/null"#;

/// Starts the guard inside the browser's process group, so killing the group
/// on a normal close takes the guard too.
#[cfg(unix)]
fn spawn_guard(group: u32) -> std::io::Result<tokio::process::Child> {
    tokio::process::Command::new("/bin/sh")
        .args(["-c", GUARD, "guard", &group.to_string()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(group as i32)
        .kill_on_drop(true)
        .spawn()
}

/// A job that ends every process in it when its last handle closes, which
/// Windows does however Themis exits.
#[cfg(windows)]
#[derive(Debug)]
struct KillOnClose(windows_sys::Win32::Foundation::HANDLE);

// SAFETY: a job handle may be used and closed from any thread.
#[cfg(windows)]
unsafe impl Send for KillOnClose {}
#[cfg(windows)]
unsafe impl Sync for KillOnClose {}

#[cfg(windows)]
impl KillOnClose {
    /// Puts `process` (and the helpers it starts) in a new job.
    fn around(process: std::os::windows::io::RawHandle) -> std::io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        // SAFETY: plain Win32 calls on a job this function creates and owns;
        // the limits struct outlives the call that reads it.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let job = Self(job);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
                || AssignProcessToJobObject(job.0, process.cast()) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(job)
        }
    }
}

#[cfg(windows)]
impl Drop for KillOnClose {
    fn drop(&mut self) {
        // SAFETY: the handle is ours and closed once; closing it ends the job.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// A running browser and its protocol connection.
#[derive(Debug)]
pub struct BrowserProcess {
    child: tokio::process::Child,
    /// Holds the write end of the guard's pipe for as long as Themis runs.
    #[cfg(unix)]
    _guard: tokio::process::Child,
    /// Ends the browser and its helpers when Themis exits, however it exits.
    #[cfg(windows)]
    _job: KillOnClose,
    pub cdp: Arc<Cdp>,
    /// `127.0.0.1:<port>` of the debugging server (for DevTools).
    pub http_address: String,
    pub executable: PathBuf,
}

impl BrowserProcess {
    /// Starts `executable` with `profile` and connects to it.
    pub async fn launch(executable: &Path, profile: &Path, headless: bool) -> anyhow::Result<Self> {
        std::fs::create_dir_all(profile)?;
        // A crashed earlier browser leaves its lock; the profile is ours alone.
        for stale in [
            "SingletonLock",
            "SingletonSocket",
            "SingletonCookie",
            "DevToolsActivePort",
        ] {
            let _ = std::fs::remove_file(profile.join(stale));
        }
        let mut command = tokio::process::Command::new(executable);
        command
            .args(launch_args(profile, headless))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(|error| {
            anyhow::anyhow!("could not start {}: {error}", executable.display())
        })?;
        #[cfg(unix)]
        let guard = match child.id().map(spawn_guard) {
            Some(Ok(guard)) => guard,
            failed => {
                let _ = child.start_kill();
                anyhow::bail!("could not guard the browser process: {failed:?}");
            }
        };
        #[cfg(windows)]
        let job = match child.raw_handle().map(KillOnClose::around) {
            Some(Ok(job)) => job,
            failed => {
                let _ = child.start_kill();
                anyhow::bail!("could not guard the browser process: {failed:?}");
            }
        };
        let stderr = child.stderr.take().expect("piped stderr");
        let address = tokio::time::timeout(START_TIMEOUT, async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(address) = devtools_address(&line) {
                    // Keep draining so the browser never blocks on a full pipe.
                    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
                    return Some(address);
                }
            }
            None
        })
        .await
        .ok()
        .flatten();
        let Some(address) = address else {
            let _ = child.start_kill();
            anyhow::bail!(
                "{} did not start its debugging server within {} s",
                executable.display(),
                START_TIMEOUT.as_secs()
            );
        };
        let cdp = Cdp::connect(&address).await?;
        let http_address = address
            .trim_start_matches("ws://")
            .split('/')
            .next()
            .unwrap_or_default()
            .to_owned();
        Ok(Self {
            child,
            #[cfg(unix)]
            _guard: guard,
            #[cfg(windows)]
            _job: job,
            cdp,
            http_address,
            executable: executable.to_path_buf(),
        })
    }

    /// The browser's process id (also its process group on Unix).
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Whether the browser process is still running.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None)) && !self.cdp.is_closed()
    }
}

impl Drop for BrowserProcess {
    fn drop(&mut self) {
        // Renderer and GPU helpers share the browser's process group.
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            // SAFETY: plain signal to the process group this handle started.
            unsafe {
                libc::killpg(pid as libc::pid_t, libc::SIGTERM);
            }
        }
        let _ = self.child.start_kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn the_guard_ends_the_group_in_every_posix_shell() {
        use std::os::unix::process::CommandExt;
        for shell in ["/bin/sh", "/bin/dash", "/bin/bash"] {
            if !Path::new(shell).exists() {
                continue;
            }
            let mut group = std::process::Command::new("sleep")
                .arg("30")
                .process_group(0)
                .spawn()
                .unwrap();
            // Its stdin is closed at once, as when Themis dies.
            let status = std::process::Command::new(shell)
                .args(["-c", GUARD, "guard", &group.id().to_string()])
                .stdin(std::process::Stdio::null())
                .status()
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while group.try_wait().unwrap().is_none() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "{shell} left the group running (guard exit {status})"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    #[test]
    fn devtools_address_accepts_only_local_browser_endpoints() {
        assert_eq!(
            devtools_address("DevTools listening on ws://127.0.0.1:53211/devtools/browser/0b6c-11"),
            Some("ws://127.0.0.1:53211/devtools/browser/0b6c-11".into())
        );
        for bad in [
            "DevTools listening on ws://evil.example:1/devtools/browser/x",
            "ws://127.0.0.1:99999/devtools/browser/x",
            "ws://127.0.0.1:9222/devtools/page/x",
            "nothing here",
        ] {
            assert_eq!(devtools_address(bad), None, "{bad}");
        }
    }

    #[test]
    fn launch_uses_its_own_profile_and_never_the_keychain() {
        let args = launch_args(Path::new("/tmp/profile"), true);
        assert!(args.contains(&"--user-data-dir=/tmp/profile".to_owned()));
        assert!(args.contains(&"--use-mock-keychain".to_owned()));
        assert!(args.contains(&"--headless=new".to_owned()));
        assert!(!launch_args(Path::new("/p"), false).contains(&"--headless=new".to_owned()));
    }

    #[test]
    fn a_configured_browser_must_exist() {
        assert_eq!(find_browser(Some(Path::new("/no/such/chrome")), None), None);
        let file = tempfile::NamedTempFile::new().unwrap();
        assert_eq!(
            find_browser(Some(file.path()), None),
            Some(file.path().to_path_buf())
        );
        assert_eq!(
            find_browser(None, Some(file.path())),
            Some(file.path().to_path_buf())
        );
    }
}
