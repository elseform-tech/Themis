//! The app's own `themis` is on every agent run's `PATH`, on every OS (CI runs this on macOS,
//! Linux and Windows): the folder a run's credential names holds `themis` (a link on macOS and
//! Linux, a copy of the console CLI on Windows), a bare `themis` resolves there first, and it
//! reaches the live app server with the run's credential. In-process app server, isolated app
//! data, synthetic credentials only.
use std::path::PathBuf;
use std::time::Duration;
use themis_desktop::server::Server;
use themis_desktop::state::AppState;

#[tokio::test(flavor = "multi_thread")]
async fn a_bare_themis_on_the_agent_path_is_the_apps_cli() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().canonicalize().unwrap().join("app-data");
    std::fs::create_dir_all(&data).unwrap();
    let state = AppState::new_for_test(data.join("settings.json"));
    let server = Server::bind(state, &data)
        .await
        .unwrap()
        .with_agent_cli(PathBuf::from(env!("CARGO_BIN_EXE_themis")));
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = tokio::spawn(server.run_until(async {
        let _ = stopped.await;
    }));
    let scope = themis_core::agent_cli::RunScope::default();
    let mut access = None;
    for _ in 0..100 {
        access = themis_core::agent_cli::issue("chat", "run", &scope);
        if access.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let access = access.expect("the server issues run credentials");
    let folder = access.bin_dir().expect("a folder with the app's themis");
    assert_eq!(folder, data.join("agent-cli"));
    let cli = folder.join(format!("themis{}", std::env::consts::EXE_SUFFIX));
    assert!(cli.is_file(), "{} is missing", cli.display());
    // Only `themis` is there: nothing else shadows the person's programs.
    let names: Vec<_> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");

    // A bare `themis`, resolved through a PATH with that folder first (as a run's shell has it),
    // answers with the run's credential.
    let path = std::env::join_paths(std::iter::once(folder.to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let output = std::process::Command::new("themis")
        .args(["plugin", "list"])
        .env("PATH", &path)
        .env_remove("OPENCODE_KEY")
        .envs(access.env())
        .output()
        .expect("`themis` is found on the agent's PATH");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "[]");
    let version = std::process::Command::new("themis")
        .arg("--version")
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("themis "));

    let _ = stop.send(());
    served.await.unwrap().unwrap();
}
