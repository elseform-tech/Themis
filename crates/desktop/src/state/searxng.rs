//! The SearXNG that ships inside Themis (built by `scripts/bundle-searxng.mjs`): started in
//! the background with the app server on a private 127.0.0.1 port, used by `web_search` when
//! no `web.search_url` is set, and stopped with the server. Its settings and log live in the
//! app-data `searxng/` folder; nothing is installed system-wide.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// How long SearXNG may take to answer its health check after starting.
const START_LIMIT: Duration = Duration::from_secs(45);

/// What the bootstrap runs: SearXNG's web app.
const WEBAPP: &str = "runpy.run_module('searx.webapp', run_name='__main__')";

/// The server's process id, passed to the bootstrap: a server that died while Python was still
/// starting has already left it with a new parent, so its own `getppid()` cannot be trusted.
const PARENT_ENV: &str = "THEMIS_SEARXNG_PARENT";

/// Runs `main` and exits when the process that started it (named by [`PARENT_ENV`]) goes away,
/// so an app that was killed never leaves SearXNG running.
fn bootstrap(main: &str) -> String {
    format!(
        "import os, threading, time, runpy\n\
parent = int(os.environ.get('{PARENT_ENV}') or os.getppid())\n\
def watch():\n    while True:\n        if os.getppid() != parent:\n            os._exit(0)\n        time.sleep(2)\n\
if os.name != 'nt':\n    threading.Thread(target=watch, daemon=True).start()\n\
{main}\n"
    )
}

/// The running built-in server, or why it is not running.
#[derive(Default)]
struct BuiltIn {
    child: Option<std::process::Child>,
    url: Option<String>,
    error: Option<String>,
}

fn built_in() -> &'static Mutex<BuiltIn> {
    static BUILT_IN: OnceLock<Mutex<BuiltIn>> = OnceLock::new();
    BUILT_IN.get_or_init(Default::default)
}

/// Where the bundled runtime is: `THEMIS_SEARXNG_DIR`, the app's resources (macOS
/// `Contents/Resources/searxng`, Windows beside the executable, Linux under its lib folder), or,
/// in debug builds only, crates/desktop/searxng-runtime once `scripts/bundle-searxng.mjs` has
/// packed it, so local builds search like the shipped app.
pub fn runtime_dir() -> Option<PathBuf> {
    let has_python = |dir: &Path| python(dir).is_file();
    if let Some(dir) = std::env::var_os("THEMIS_SEARXNG_DIR").map(PathBuf::from) {
        return has_python(&dir).then_some(dir);
    }
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let mut candidates = vec![
        exe_dir.join("../Resources/searxng"),
        exe_dir.join("searxng"),
        exe_dir.join("../lib/ThemisCode/searxng"),
        exe_dir.join("../lib/themis-code/searxng"),
    ];
    if cfg!(debug_assertions) {
        candidates.push(development_runtime());
    }
    candidates.into_iter().find(|dir| has_python(dir))
}

/// The runtime `scripts/bundle-searxng.mjs` packs in this source tree.
fn development_runtime() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("searxng-runtime")
}

fn python(dir: &Path) -> PathBuf {
    if cfg!(windows) {
        dir.join("python").join("python.exe")
    } else {
        dir.join("python").join("bin").join("python3")
    }
}

/// The built-in server's address once it answers, starting it if needed. `None` with a
/// reason when Themis has no bundled SearXNG or it could not start.
pub async fn url(app_dir: &Path) -> Result<String, String> {
    if let Some(url) = running_url() {
        return Ok(url);
    }
    let app_dir = app_dir.to_owned();
    tokio::task::spawn_blocking(move || start(&app_dir))
        .await
        .map_err(|e| e.to_string())?
}

/// Longest a run waits for the built-in server before it starts without it.
pub const RUN_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// How a run's wait for the search server ended.
#[derive(Debug, PartialEq, Eq)]
pub enum ForRun {
    Ready(String),
    /// It failed to start; why.
    Unavailable(String),
    /// Still starting after `wait`: this run goes without it, later runs get it.
    Starting,
}

/// Waits for `address` at most `wait`, so a search server that is slow or never answers can't
/// hold a run up (it used to wait up to 45 s before the run began).
pub async fn for_run(
    wait: std::time::Duration,
    address: impl std::future::Future<Output = Result<String, String>>,
) -> ForRun {
    match tokio::time::timeout(wait, address).await {
        Ok(Ok(url)) => ForRun::Ready(url),
        Ok(Err(error)) => ForRun::Unavailable(error),
        Err(_) => ForRun::Starting,
    }
}

/// Starts it in the background when the app server starts, so the first search is quick.
pub fn start_in_background(app_dir: &Path) {
    if runtime_dir().is_none() {
        return;
    }
    let app_dir = app_dir.to_owned();
    std::thread::spawn(move || {
        let _ = start(&app_dir);
    });
}

/// Whether the built-in server is part of this build, and its address or error.
pub fn status() -> serde_json::Value {
    let state = built_in().lock().unwrap_or_else(|e| e.into_inner());
    serde_json::json!({
        "available": runtime_dir().is_some(),
        "url": state.url,
        "error": state.error,
    })
}

fn running_url() -> Option<String> {
    let mut state = built_in().lock().unwrap_or_else(|e| e.into_inner());
    let alive = state
        .child
        .as_mut()
        .is_some_and(|child| child.try_wait().ok().flatten().is_none());
    if !alive {
        state.child = None;
        state.url = None;
    }
    state.url.clone()
}

fn start(app_dir: &Path) -> Result<String, String> {
    static STARTING: Mutex<()> = Mutex::new(());
    let _one_at_a_time = STARTING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(url) = running_url() {
        return Ok(url);
    }
    let result = launch(app_dir);
    let mut state = built_in().lock().unwrap_or_else(|e| e.into_inner());
    match result {
        Ok((child, url)) => {
            state.child = Some(child);
            state.url = Some(url.clone());
            state.error = None;
            Ok(url)
        }
        Err(error) => {
            state.error = Some(error.clone());
            Err(error)
        }
    }
}

fn launch(app_dir: &Path) -> Result<(std::process::Child, String), String> {
    let runtime = runtime_dir().ok_or("This build of Themis has no built-in search server")?;
    let folder = app_dir.join("searxng");
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map_err(|e| e.to_string())?
        .port();
    let settings = folder.join("settings.yml");
    std::fs::write(&settings, settings_yml(&secret(&folder)?, port)).map_err(|e| e.to_string())?;
    let log = std::fs::File::create(folder.join("searxng.log")).map_err(|e| e.to_string())?;
    let mut command = std::process::Command::new(python(&runtime));
    command
        .args(["-I", "-c", &bootstrap(WEBAPP)])
        .current_dir(&folder)
        .env("SEARXNG_SETTINGS_PATH", &settings)
        .env(PARENT_ENV, std::process::id().to_string())
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console window for the background server.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("The built-in search server could not start: {e}"))?;
    let url = format!("http://127.0.0.1:{port}");
    let deadline = std::time::Instant::now() + START_LIMIT;
    while std::time::Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "The built-in search server stopped ({status}); see searxng/searxng.log"
            ));
        }
        if healthy(port) {
            return Ok((child, url));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = child.kill();
    let _ = child.wait();
    Err("The built-in search server did not answer in time; see searxng/searxng.log".into())
}

/// One plain HTTP request to `/healthz`, without pulling in a client for it.
fn healthy(port: u16) -> bool {
    use std::io::{Read, Write};
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(300),
    ) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    if stream
        .write_all(b"GET /healthz HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut head = [0u8; 12];
    stream.read_exact(&mut head).is_ok() && head.starts_with(b"HTTP/1.") && &head[9..12] == b"200"
}

/// The instance's secret key, made once and kept (SearXNG refuses its placeholder).
fn secret(folder: &Path) -> Result<String, String> {
    let path = folder.join("secret");
    if let Ok(secret) = std::fs::read_to_string(&path) {
        if secret.trim().len() >= 32 {
            return Ok(secret.trim().to_owned());
        }
    }
    let secret = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    std::fs::write(&path, &secret).map_err(|e| e.to_string())?;
    Ok(secret)
}

/// Bound to this computer only, JSON output on, no rate limiter or image proxy. The YouTube
/// engine is never loaded: the bundle leaves out its module, which carries YouTube's public
/// web-client key that secret scanners flag.
fn settings_yml(secret: &str, port: u16) -> String {
    format!(
        "use_default_settings: true\n\
general:\n  instance_name: \"Themis search\"\n\
server:\n  secret_key: \"{secret}\"\n  bind_address: \"127.0.0.1\"\n  port: {port}\n  limiter: false\n  image_proxy: false\n  public_instance: false\n\
search:\n  formats: [html, json]\n\
engines:\n  - name: youtube\n    inactive: true\n"
    )
}

/// Stops the built-in server (the app server is exiting).
pub fn stop() {
    let mut state = built_in().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut child) = state.child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    state.url = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Local builds find the runtime the bundle script packs in the source tree; a release
    /// build only ever looks at its own resources.
    #[test]
    fn debug_builds_look_in_the_source_tree() {
        assert!(development_runtime().ends_with("crates/desktop/searxng-runtime"));
        if std::env::var_os("THEMIS_SEARXNG_DIR").is_none()
            && python(&development_runtime()).is_file()
        {
            assert_eq!(runtime_dir(), Some(development_runtime()));
        }
    }

    #[cfg(unix)]
    fn alive(pid: &str) -> bool {
        std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// The server can die while SearXNG's Python is still starting (a crash, `kill -9`, a test
    /// that stops at once): Python then never sees its real parent, yet must still exit.
    #[cfg(unix)]
    #[test]
    fn search_exits_when_its_server_died_before_python_started() {
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipped: no python3");
            return;
        }
        let script = bootstrap("time.sleep(120)");
        // A stand-in server (`sh`) starts the bootstrap the way `launch` does, naming itself
        // as the parent, and exits before Python has started.
        let started = std::process::Command::new("sh")
            .args([
                "-c",
                "export \"$2=$$\"; \
                 (sleep 0.5; exec python3 -I -c \"$1\") </dev/null >/dev/null 2>&1 & echo $!",
                "sh",
                &script,
                PARENT_ENV,
            ])
            .output()
            .unwrap();
        let pid = String::from_utf8_lossy(&started.stdout).trim().to_owned();
        assert!(!pid.is_empty());
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while alive(&pid) {
            if std::time::Instant::now() > deadline {
                let _ = std::process::Command::new("kill").arg(&pid).status();
                panic!("search outlived a server that died before it started");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// While the server that started it runs, it keeps running.
    #[cfg(unix)]
    #[test]
    fn search_keeps_running_while_its_server_runs() {
        let Ok(mut child) = std::process::Command::new("python3")
            .args(["-I", "-c", &bootstrap("time.sleep(120)")])
            .env(PARENT_ENV, std::process::id().to_string())
            .spawn()
        else {
            eprintln!("skipped: no python3");
            return;
        };
        std::thread::sleep(Duration::from_secs(3));
        let running = child.try_wait().unwrap().is_none();
        let _ = child.kill();
        let _ = child.wait();
        assert!(running, "search stopped while its server was running");
    }

    #[test]
    fn settings_bind_locally_with_json_on() {
        let yml = settings_yml("s".repeat(64).as_str(), 4242);
        assert!(yml.contains("bind_address: \"127.0.0.1\"") && yml.contains("port: 4242"));
        assert!(yml.contains("formats: [html, json]") && yml.contains("limiter: false"));
        // The bundle leaves out the YouTube engine's module, so it must never be loaded.
        assert!(
            yml.ends_with("engines:\n  - name: youtube\n    inactive: true\n"),
            "{yml}"
        );
    }

    #[test]
    fn the_secret_is_made_once() {
        let folder = tempfile::tempdir().unwrap();
        let first = secret(folder.path()).unwrap();
        assert_eq!(secret(folder.path()).unwrap(), first);
        assert!(first.len() >= 32);
    }

    #[tokio::test]
    async fn a_run_waits_for_the_search_server_only_briefly() {
        let wait = std::time::Duration::from_millis(100);
        let started = std::time::Instant::now();
        let slow = async {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            Ok::<_, String>("http://127.0.0.1:1".to_owned())
        };
        assert_eq!(for_run(wait, slow).await, ForRun::Starting);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        let ready = async { Ok::<_, String>("http://127.0.0.1:2".to_owned()) };
        assert_eq!(
            for_run(wait, ready).await,
            ForRun::Ready("http://127.0.0.1:2".into())
        );
        let failed = async { Err::<String, _>("no python".to_owned()) };
        assert_eq!(
            for_run(wait, failed).await,
            ForRun::Unavailable("no python".into())
        );
    }
}
