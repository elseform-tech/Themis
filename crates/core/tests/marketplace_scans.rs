use serde_json::json;
use std::{fs, path::Path};
use themis_core::plugins::PluginStore;

fn fixture(root: &Path) {
    fs::create_dir_all(root.join(".claude-plugin")).unwrap();
    fs::create_dir_all(root.join("good")).unwrap();
    fs::write(
        root.join("good/SKILL.md"),
        "---\nname: good\ndescription: Review\n---\nOriginal instructions",
    )
    .unwrap();
    fs::write(root.join(".claude-plugin/marketplace.json"), json!({"plugins":[{"name":"good","source":"./good"},{"name":"broken","source":"./missing"}]}).to_string()).unwrap();
}

#[tokio::test]
async fn background_scans_share_a_persisted_snapshot_and_guard_refresh_races() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    fixture(source.path());
    let store = PluginStore::new(data.path().into(), None);
    store
        .add_marketplace("test", source.path().to_str().unwrap())
        .unwrap();
    let initial = store.start_marketplace_scan("test", false).unwrap();
    assert_eq!(initial.status, "scanning");
    let duplicate = store.start_marketplace_scan("test", false).unwrap();
    assert_eq!(initial.revision, duplicate.revision);
    let manual = store.clone();
    let preview_task =
        tokio::spawn(async move { manual.scanned_marketplace_preview("test", "good").await });
    assert!(store.remove_marketplace("test").is_err());
    let done = loop {
        let state = store.start_marketplace_scan("test", false).unwrap();
        if state.status != "scanning" {
            break state;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    assert_eq!(done.status, "ready");
    assert!(preview_task.await.unwrap().is_ok());
    assert_eq!(done.completed, 2);
    assert_eq!(done.errors.len(), 1);
    assert!(store
        .scanned_marketplace_preview("test", "broken")
        .await
        .is_err());
    fs::remove_file(source.path().join("good/SKILL.md")).unwrap();
    let restarted = PluginStore::new(data.path().into(), None);
    assert_eq!(
        restarted
            .start_marketplace_scan("test", false)
            .unwrap()
            .revision,
        done.revision
    );
    assert!(restarted
        .scanned_marketplace_preview("test", "good")
        .await
        .unwrap()
        .spec
        .skills[0]
        .instructions
        .contains("Original"));
    assert!(restarted
        .install_scanned("global", "test", "good", false, Some("stale"))
        .await
        .is_err());
    let installed = restarted
        .install_scanned("global", "test", "good", false, Some(&done.revision))
        .await
        .unwrap();
    assert_eq!(installed.spec.skills.len(), 1);
    let refresh = restarted.start_marketplace_scan("test", true).unwrap();
    assert_ne!(refresh.revision, done.revision);
    assert!(restarted
        .install_scanned("global", "test", "good", false, Some(&done.revision))
        .await
        .is_err());
    loop {
        if restarted
            .start_marketplace_scan("test", false)
            .unwrap()
            .status
            != "scanning"
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        restarted
            .scanned_marketplace_preview("test", "good")
            .await
            .unwrap()
            .report
            .status,
        "unsupported"
    );
    assert!(restarted
        .install_scanned("global", "test", "good", true, None)
        .await
        .is_err());
    restarted.remove_marketplace("test").unwrap();
    assert!(restarted.start_marketplace_scan("test", false).is_err());
    let replacement = tempfile::tempdir().unwrap();
    fixture(replacement.path());
    fs::write(
        replacement.path().join("good/SKILL.md"),
        "---\nname: good\ndescription: Review\n---\nReplacement source",
    )
    .unwrap();
    restarted
        .add_marketplace("test", replacement.path().to_str().unwrap())
        .unwrap();
    let new_scan = restarted
        .wait_marketplace_scan("test", false)
        .await
        .unwrap();
    assert_ne!(new_scan.revision, done.revision);
    assert!(restarted
        .scanned_marketplace_preview("test", "good")
        .await
        .unwrap()
        .spec
        .skills[0]
        .instructions
        .contains("Replacement source"));
}

#[test]
fn interrupted_scan_resumes_without_changing_its_revision() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    fixture(source.path());
    let store = PluginStore::new(data.path().into(), None);
    store
        .add_marketplace("resume", source.path().to_str().unwrap())
        .unwrap();
    let first = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let pending = first.block_on(async { store.start_marketplace_scan("resume", false).unwrap() });
    drop(first); // Cancels the task and releases its OS lock, as a server exit would.
    let next = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    next.block_on(async {
        let finished = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            store.wait_marketplace_scan("resume", false),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(pending.revision, finished.revision);
        assert_eq!(finished.completed, 2);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_manual_previews_join_the_scan_without_transient_busy_failures() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    fixture(source.path());
    let store = PluginStore::new(data.path().into(), None);
    store
        .add_marketplace("concurrent", source.path().to_str().unwrap())
        .unwrap();
    let scan = store.start_marketplace_scan("concurrent", false).unwrap();
    let mut readers = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let store = store.clone();
        readers.spawn(async move {
            let state = store
                .wait_marketplace_scan("concurrent", false)
                .await
                .unwrap();
            let preview = store
                .scanned_marketplace_preview("concurrent", "good")
                .await
                .unwrap();
            (state.revision, preview.report.status)
        });
    }
    while let Some(result) = readers.join_next().await {
        let (revision, status) = result.unwrap();
        assert_eq!(revision, scan.revision);
        assert_eq!(status, "supported");
    }
}

#[tokio::test]
async fn previous_instruction_limit_snapshot_is_rechecked() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    fixture(source.path());
    let store = PluginStore::new(data.path().into(), None);
    store
        .add_marketplace("limits", source.path().to_str().unwrap())
        .unwrap();
    let before = store.wait_marketplace_scan("limits", false).await.unwrap();
    let path = data.path().join("compatibility-cache/limits/scan.json");
    let mut old: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    old["version"] = 1.into();
    std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
    let after = store.wait_marketplace_scan("limits", false).await.unwrap();
    assert_ne!(before.revision, after.revision);
    assert_eq!(after.status, "ready");
}

#[tokio::test]
async fn failed_scan_reports_the_underlying_reason_not_just_the_component() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    fixture(source.path());
    fs::write(
        source.path().join("good/.mcp.json"),
        r#"{"mcpServers":{"example":{"command":"node","env":{"TOKEN":"synthetic-literal"}}}}"#,
    )
    .unwrap();
    let store = PluginStore::new(data.path().into(), None);
    store
        .add_marketplace("reasons", source.path().to_str().unwrap())
        .unwrap();
    let scan = store.wait_marketplace_scan("reasons", false).await.unwrap();
    assert!(
        scan.errors["good"].contains("literal values are unsupported"),
        "{}",
        scan.errors["good"]
    );
    assert!(!scan.errors["good"].contains("synthetic-literal"));
}
