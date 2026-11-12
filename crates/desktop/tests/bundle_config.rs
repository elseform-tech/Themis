//! The Windows bundle ships the console `themis` CLI as an external binary: `tauri-build` copies
//! `binaries/themis-cli-<triple>.exe` next to the app in the target directory, and CI checks that
//! copy. These guard the two things that copy depends on.

use std::path::Path;

use serde_json::Value;

fn read_json(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
        .unwrap_or_else(|error| panic!("{name}: {error}"))
}

/// tauri-utils merges the platform file over `tauri.conf.json` as a JSON merge patch.
fn merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(base), Value::Object(patch)) => {
            for (key, value) in patch {
                if value.is_null() {
                    base.remove(key);
                } else {
                    merge(base.entry(key.clone()).or_insert(Value::Null), value);
                }
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

#[test]
fn the_windows_config_still_ships_the_cli_as_an_external_binary() {
    let mut config = read_json("tauri.conf.json");
    merge(&mut config, &read_json("tauri.windows.conf.json"));
    assert_eq!(
        config["bundle"]["externalBin"],
        serde_json::json!(["binaries/themis-cli"])
    );
    assert!(config["identifier"].is_string());
}

/// Cargo 1.100 moved a build script's `OUT_DIR` one level deeper
/// (`target/<profile>/build/<pkg>/<hash>/out`). tauri-build before 2.7 found the target directory
/// as `OUT_DIR`'s third parent, so it copied `themis-cli.exe` into `target/<profile>/build/` and
/// the bundle check found nothing. 2.7 walks up to the `build` directory instead.
#[test]
fn tauri_build_finds_the_target_dir_under_the_new_build_layout() {
    let lock =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock"))
            .unwrap()
            // A Windows checkout may have CRLF line endings.
            .replace("\r\n", "\n");
    let version = lock
        .split("\n[[package]]\n")
        .find(|entry| entry.starts_with("name = \"tauri-build\"\n"))
        .and_then(|entry| entry.lines().nth(1))
        .and_then(|line| line.strip_prefix("version = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .expect("tauri-build in Cargo.lock");
    let parts: Vec<u32> = version
        .split('.')
        .map(|part| part.parse().unwrap())
        .collect();
    assert!(
        (parts[0], parts[1]) >= (2, 7),
        "tauri-build {version} misplaces external binaries under cargo 1.100's build layout"
    );
}
