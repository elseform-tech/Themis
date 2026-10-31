//! Measures the real official catalog through the shared server and the `themis` CLI, with the
//! test state: secrets stay in memory (never the Keychain), the data folder is the one given and
//! personal skill folders are not read. It clones third-party repositories, so it is opt-in:
//!
//! ```sh
//! THEMIS_CATALOG_DATA=/some/empty/folder \
//!   cargo test -p themis-desktop --test catalog_check_live -- --ignored --nocapture
//! ```
//!
//! The data folder is kept: `compatibility-cache/claude-plugins-official/` holds each plugin's
//! saved check, for comparing two runs plugin by plugin.

use themis_desktop::{server::Server, state::AppState};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs network; clones every plugin in the official catalog"]
async fn official_catalog_counts() {
    let data = std::path::PathBuf::from(
        std::env::var_os("THEMIS_CATALOG_DATA").expect("set THEMIS_CATALOG_DATA to a folder"),
    );
    std::fs::create_dir_all(&data).unwrap();
    let state = AppState::new_for_test(data.join("settings.json"));
    let server = tokio::spawn(Server::bind(state, &data).await.unwrap().run());
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(&data)
        .args(["plugin", "marketplace", "check", "claude-plugins-official"])
        .env_remove("OPENCODE_KEY")
        .output()
        .await
        .unwrap();
    server.abort();
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
