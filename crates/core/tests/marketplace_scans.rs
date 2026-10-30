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
    let check = done.checks["good"].check_revision.clone().unwrap();
    let installed = restarted
        .install_scanned("global", "test", "good", false, Some(&check))
        .await
        .unwrap();
    assert_eq!(installed.spec.skills.len(), 1);
    let refresh = restarted.start_marketplace_scan("test", true).unwrap();
    assert_ne!(refresh.revision, done.revision);
    assert!(restarted
        .install_scanned("global", "test", "good", false, Some(&check))
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

/// A named catalog with two plugins.
fn acme_catalog(root: &Path) {
    fs::create_dir_all(root.join(".claude-plugin")).unwrap();
    fs::create_dir_all(root.join("quick")).unwrap();
    fs::write(
        root.join("quick/SKILL.md"),
        "---\nname: quick\ndescription: Quick\n---\nQuick instructions",
    )
    .unwrap();
    fs::write(
        root.join(".claude-plugin/marketplace.json"),
        json!({"name":"acme-plugins","owner":{"name":"Acme Inc"},"plugins":[
            {"name":"quick","source":"./quick"},
            {"name":"another","source":"./quick"}
        ]})
        .to_string(),
    )
    .unwrap();
}

#[tokio::test]
async fn a_checked_entry_installs_from_its_own_check_while_the_catalog_is_still_scanning() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    acme_catalog(source.path());
    let store = PluginStore::new(data.path().into(), None);
    store
        .add_marketplace("test", source.path().to_str().unwrap())
        .unwrap();
    // Another writer holds the scan lock and has saved one finished entry, as a scan in progress
    // does. Nothing here may wait for it.
    let done = store.wait_marketplace_scan("test", false).await.unwrap();
    let scan_file = data.path().join("compatibility-cache/test/scan.json");
    let mut saved: serde_json::Value =
        serde_json::from_slice(&fs::read(&scan_file).unwrap()).unwrap();
    saved["status"] = "scanning".into();
    saved["checks"].as_object_mut().unwrap().remove("another");
    fs::write(&scan_file, saved.to_string()).unwrap();
    let lock = fs::OpenOptions::new()
        .write(true)
        .open(data.path().join("compatibility-cache/test/scan.lock"))
        .unwrap();
    lock.lock().unwrap();
    let check = done.checks["quick"].check_revision.clone().unwrap();
    let installed = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        store.install_scanned("global", "test", "quick", false, Some(&check)),
    )
    .await
    .expect("must not wait for the rest of the catalog")
    .unwrap();
    assert_eq!(installed.spec.name, "quick");
    // A reviewed check that has since been replaced stops the install.
    let error = store
        .install_scanned("global", "test", "quick", false, Some("older-check"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("reopen"), "{error}");
    lock.unlock().unwrap();
}

#[tokio::test]
async fn a_local_source_previews_then_adds_under_its_manifest_name_and_can_be_hidden() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    acme_catalog(source.path());
    let store = PluginStore::new(data.path().into(), None);
    let path = source.path().to_str().unwrap();
    let preview = store.preview_source(path).await.unwrap();
    assert_eq!(preview.name, "acme-plugins");
    assert_eq!(preview.publisher, "Acme Inc");
    assert_eq!(preview.plugins, 2);
    // Previewing adds nothing.
    assert!(!store
        .marketplaces()
        .unwrap()
        .iter()
        .any(|m| m.name == "acme-plugins"));
    assert_eq!(store.add_source(path).await.unwrap(), "acme-plugins");
    assert!(store.add_source(path).await.is_err(), "added twice");
    store
        .set_marketplace_enabled("acme-plugins", false)
        .unwrap();
    let sources = store.sources().unwrap();
    let added = sources.iter().find(|s| s.name == "acme-plugins").unwrap();
    assert!(!added.enabled && !added.official);
    assert!(store.sources().unwrap().iter().any(|s| s.official));
    // Removing the source keeps what was installed from it; the official one stays.
    store
        .wait_marketplace_scan("acme-plugins", false)
        .await
        .unwrap();
    store
        .install_scanned("global", "acme-plugins", "quick", false, None)
        .await
        .unwrap();
    store.remove_marketplace("acme-plugins").unwrap();
    assert!(store.list().unwrap().iter().any(|p| p.spec.name == "quick"));
    let official_error = store
        .remove_marketplace("claude-plugins-official")
        .unwrap_err();
    assert!(
        official_error.to_string().contains("can't be removed"),
        "{official_error}"
    );
}

#[tokio::test]
async fn sources_that_are_not_marketplaces_say_so_in_one_line() {
    let data = tempfile::tempdir().unwrap();
    let store = PluginStore::new(data.path().into(), None);
    let empty = tempfile::tempdir().unwrap();
    let error = store
        .preview_source(empty.path().to_str().unwrap())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "No marketplace found in this source.");
    fs::create_dir_all(empty.path().join(".claude-plugin")).unwrap();
    let manifest = empty.path().join(".claude-plugin/marketplace.json");
    fs::write(&manifest, "not json").unwrap();
    let error = store
        .preview_source(empty.path().to_str().unwrap())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "No marketplace found in this source.");
    let too_many: Vec<_> = (0..2001)
        .map(|n| json!({"name": format!("p{n}")}))
        .collect();
    fs::write(
        &manifest,
        json!({"name":"big","plugins":too_many}).to_string(),
    )
    .unwrap();
    let error = store
        .preview_source(empty.path().to_str().unwrap())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "This marketplace has more than 2,000 plugins."
    );
    assert!(store.preview_source("not a source").await.is_err());
    assert!(store
        .preview_source("https://user:secret@example.com/a.git")
        .await
        .is_err());
}
