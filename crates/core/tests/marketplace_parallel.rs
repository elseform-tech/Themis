#![cfg(unix)]
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt};
use themis_core::plugins::PluginStore;

// A separate test process isolates PATH. Only network cloning is replaced; real
// inspection, validation, snapshot writes and installation run unchanged.
#[tokio::test]
async fn marketplace_checks_overlap_with_a_bounded_number_of_clones() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let git = bin.path().join("git");
    fs::write(
        &git,
        r#"#!/usr/bin/env python3
import fcntl, json, os, pathlib, sys, time
state = pathlib.Path(__file__).with_name("counts.json")
def count(delta):
    with state.open("a+") as f:
        fcntl.flock(f, fcntl.LOCK_EX)
        f.seek(0)
        value = json.loads(f.read() or '[0,0]')
        value[0] += delta
        value[1] = max(value)
        f.seek(0)
        f.truncate()
        json.dump(value, f)
if "clone" in sys.argv:
    count(1)
    time.sleep(.2)
    path = pathlib.Path(sys.argv[-1])
    path.mkdir(parents=True)
    (path / "SKILL.md").write_text("---\nname: review\ndescription: Review\n---\nRead carefully.")
    count(-1)
"#,
    )
    .unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
    let counts = bin.path().join("counts.json");
    std::env::set_var(
        "PATH",
        format!(
            "{}:{}",
            bin.path().display(),
            std::env::var("PATH").unwrap()
        ),
    );
    fs::create_dir(source.path().join(".claude-plugin")).unwrap();
    let entries: Vec<_> = (0..9).map(|i| json!({"name":format!("plugin-{i}"),"source":{"source":"url","url":"https://example.com/test.git"}})).collect();
    fs::write(
        source.path().join(".claude-plugin/marketplace.json"),
        json!({"plugins":entries}).to_string(),
    )
    .unwrap();
    let store = PluginStore::new(data.path().into(), None);
    store
        .add_marketplace("parallel", source.path().to_str().unwrap())
        .unwrap();
    let scan = store
        .wait_marketplace_scan("parallel", false)
        .await
        .unwrap();
    assert_eq!(scan.completed, 9);
    assert!(scan.errors.is_empty(), "{:?}", scan.errors);
    let counts: Vec<usize> = serde_json::from_slice(&fs::read(counts).unwrap()).unwrap();
    assert!(counts[1] > 1, "checks must overlap: {counts:?}");
    assert!(
        counts[1] <= 4,
        "clone concurrency must remain bounded: {counts:?}"
    );
    assert_eq!(counts[0], 0);
    let installed = store
        .install_scanned(
            "global",
            "parallel",
            "plugin-8",
            false,
            Some(&scan.revision),
        )
        .await
        .unwrap();
    assert_eq!(installed.spec.skills[0].instructions, "Read carefully.");
}
