//! The SearXNG that ships inside Themis (built by `scripts/bundle-searxng.mjs`): started in
//! the background with the app server on a private 127.0.0.1 port, used by `web_search` when
//! no `web.search_url` is set, and stopped with the server. Its settings and log live in the
//! app-data `searxng/` folder; nothing is installed system-wide.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// How long SearXNG may take to answer its health check after starting.
const START_LIMIT: Duration = Duration::from_secs(45);

/// Runs `searx.webapp` and exits when the process that started it goes away, so an app that
/// was killed never leaves SearXNG running.
const BOOTSTRAP: &str = "import os, threading, time, runpy\n\
parent = os.getppid()\n\
def watch():\n    while True:\n        time.sleep(2)\n        if os.getppid() != parent:\n            os._exit(0)\n\
if os.name != 'nt':\n    threading.Thread(target=watch, daemon=True).start()\n\
runpy.run_module('searx.webapp', run_name='__main__')\n";

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

/// Where the bundled runtime is: `THEMIS_SEARXNG_DIR` (development: point it at
/// crates/desktop/searxng-runtime), or the app's resources (macOS `Contents/Resources/searxng`,
/// Windows beside the executable, Linux under its lib folder).
pub fn runtime_dir() -> Option<PathBuf> {
    let has_python = |dir: &Path| python(dir).is_file();
    if let Some(dir) = std::env::var_os("THEMIS_SEARXNG_DIR").map(PathBuf::from) {
        return has_python(&dir).then_some(dir);
    }
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let candidates = [
        exe_dir.join("../Resources/searxng"),
        exe_dir.join("searxng"),
        exe_dir.join("../lib/ThemisCode/searxng"),
        exe_dir.join("../lib/themis-code/searxng"),
    ];
    candidates.into_iter().find(|dir| has_python(dir))
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
        .args(["-I", "-c", BOOTSTRAP])
        .current_dir(&folder)
        .env("SEARXNG_SETTINGS_PATH", &settings)
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

/// Bound to this computer only, JSON output on, no rate limiter or image proxy.
fn settings_yml(secret: &str, port: u16) -> String {
    format!(
        "use_default_settings: true\n\
general:\n  instance_name: \"Themis search\"\n\
server:\n  secret_key: \"{secret}\"\n  bind_address: \"127.0.0.1\"\n  port: {port}\n  limiter: false\n  image_proxy: false\n  public_instance: false\n\
search:\n  formats: [html, json]\n"
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

    #[test]
    fn settings_bind_locally_with_json_on() {
        let yml = settings_yml("s".repeat(64).as_str(), 4242);
        assert!(yml.contains("bind_address: \"127.0.0.1\"") && yml.contains("port: 4242"));
        assert!(yml.contains("formats: [html, json]") && yml.contains("limiter: false"));
    }

    #[test]
    fn the_secret_is_made_once() {
        let folder = tempfile::tempdir().unwrap();
        let first = secret(folder.path()).unwrap();
        assert_eq!(secret(folder.path()).unwrap(), first);
        assert!(first.len() >= 32);
    }
}
