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

#[tokio::test]
async fn only_changed_entries_are_checked_again_and_the_plugin_offers_an_update() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    acme_catalog(source.path());
    fs::create_dir_all(source.path().join("other")).unwrap();
    fs::write(
        source.path().join("other/SKILL.md"),
        "---\nname: other\ndescription: Other\n---\nOther instructions",
    )
    .unwrap();
    fs::write(
        source.path().join(".claude-plugin/marketplace.json"),
        json!({"name":"acme","plugins":[
            {"name":"quick","source":"./quick"},
            {"name":"other","source":"./other"}
        ]})
        .to_string(),
    )
    .unwrap();
    let store = PluginStore::new(data.path().into(), None);
    store
        .add_marketplace("acme", source.path().to_str().unwrap())
        .unwrap();
    let first = store.wait_marketplace_scan("acme", false).await.unwrap();
    let quick_check = first.checks["quick"].check_revision.clone();
    let other_check = first.checks["other"].check_revision.clone();
    store
        .install_scanned("global", "acme", "quick", false, None)
        .await
        .unwrap();
    assert!(store.plugin_updates().unwrap().is_empty(), "just installed");

    // Nothing upstream changed: nothing is checked again.
    assert_eq!(store.check_source_for_changes("acme").await.unwrap(), 0);
    let same = store.wait_marketplace_scan("acme", false).await.unwrap();
    assert_eq!(same.checks["quick"].check_revision, quick_check);
    assert!(store.plugin_updates().unwrap().is_empty());

    // One entry changes: only that one gets a new check, and it offers an update.
    fs::write(
        source.path().join("quick/SKILL.md"),
        "---\nname: quick\ndescription: Quick\n---\nNew instructions",
    )
    .unwrap();
    assert_eq!(store.check_source_for_changes("acme").await.unwrap(), 1);
    let changed = store.wait_marketplace_scan("acme", false).await.unwrap();
    assert_ne!(changed.checks["quick"].check_revision, quick_check);
    assert_eq!(changed.checks["other"].check_revision, other_check);
    assert_eq!(changed.status, "ready");
    let updates = store.plugin_updates().unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].name, "quick");

    // Updating installs the newly checked package and clears the offer.
    let updated = store
        .install_scanned(
            "global",
            "acme",
            "quick",
            false,
            updates[0].check_revision.as_deref(),
        )
        .await
        .unwrap();
    assert!(updated.spec.skills[0]
        .instructions
        .contains("New instructions"));
    assert!(store.plugin_updates().unwrap().is_empty());
}

#[tokio::test]
async fn an_update_turns_off_changed_parts_and_keeps_unchanged_ones_as_they_were() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    fs::create_dir_all(source.path().join(".claude-plugin")).unwrap();
    let plugin = source.path().join("guarded");
    fs::create_dir_all(plugin.join("hooks")).unwrap();
    fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
    fs::write(
        plugin.join(".claude-plugin/plugin.json"),
        r#"{"name":"guarded"}"#,
    )
    .unwrap();
    let hooks = |start: &str| {
        json!({"hooks":{
            "SessionStart":[{"hooks":[{"type":"command","command":start}]}],
            "PostToolUse":[{"hooks":[{"type":"command","command":"exit 0"}]}]
        }})
        .to_string()
    };
    fs::write(plugin.join("hooks/hooks.json"), hooks("echo one")).unwrap();
    fs::write(
        source.path().join(".claude-plugin/marketplace.json"),
        json!({"name":"acme","plugins":[{"name":"guarded","source":"./guarded"}]}).to_string(),
    )
    .unwrap();
    let store = PluginStore::new(data.path().into(), None);
    store
        .add_marketplace("acme", source.path().to_str().unwrap())
        .unwrap();
    store.wait_marketplace_scan("acme", false).await.unwrap();
    let installed = store
        .install_scanned("global", "acme", "guarded", true, None)
        .await
        .unwrap();
    for hook in &installed.spec.hooks {
        store
            .set_component_enabled("global", "guarded", "hook", &hook.name, true)
            .unwrap();
    }
    fs::write(plugin.join("hooks/hooks.json"), hooks("echo two")).unwrap();
    assert_eq!(store.check_source_for_changes("acme").await.unwrap(), 1);
    let updated = store
        .install_scanned("global", "acme", "guarded", true, None)
        .await
        .unwrap();
    let hook = |command: &str| {
        updated
            .spec
            .hooks
            .iter()
            .find(|hook| hook.command == command)
            .unwrap()
            .enabled
    };
    assert!(!hook("echo two"), "changed hook arrives off");
    assert!(hook("exit 0"), "unchanged hook stays on");
}

#[tokio::test]
async fn opening_the_app_checks_only_sources_not_checked_today_and_shown() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    acme_catalog(source.path());
    let store = PluginStore::new(data.path().into(), None);
    store
        .add_marketplace("acme", source.path().to_str().unwrap())
        .unwrap();
    store.wait_marketplace_scan("acme", false).await.unwrap();
    store
        .install_scanned("global", "acme", "quick", false, None)
        .await
        .unwrap();
    fs::write(
        source.path().join("quick/SKILL.md"),
        "---\nname: quick\ndescription: Quick\n---\nChanged",
    )
    .unwrap();
    let settle = || tokio::time::sleep(std::time::Duration::from_millis(300));
    // Checked moments ago: not due.
    store.check_sources().unwrap();
    settle().await;
    assert!(store.plugin_updates().unwrap().is_empty());
    let scan_file = data.path().join("compatibility-cache/acme/scan.json");
    let mut saved: serde_json::Value =
        serde_json::from_slice(&fs::read(&scan_file).unwrap()).unwrap();
    saved["checked_at"] = 1.into();
    fs::write(&scan_file, saved.to_string()).unwrap();
    // Due but hidden: left alone.
    store.set_marketplace_enabled("acme", false).unwrap();
    store.check_sources().unwrap();
    settle().await;
    assert!(store.plugin_updates().unwrap().is_empty());
    // Due and shown: checked in the background, and the update appears.
    store.set_marketplace_enabled("acme", true).unwrap();
    store.check_sources().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while store.plugin_updates().unwrap().is_empty() {
        assert!(std::time::Instant::now() < deadline, "no update found");
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn a_bundled_icon_is_saved_during_the_check_and_outlives_its_source() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    acme_catalog(source.path());
    let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[7; 32]].concat();
    fs::write(source.path().join("quick/logo.png"), &png).unwrap();
    fs::write(
        source.path().join(".claude-plugin/marketplace.json"),
        json!({"name":"acme","plugins":[{"name":"quick","source":"./quick","icon":"./logo.png"}]})
            .to_string(),
    )
    .unwrap();
    let store = PluginStore::new(data.path().into(), None);
    store
        .add_marketplace("acme", source.path().to_str().unwrap())
        .unwrap();
    let scan = store.wait_marketplace_scan("acme", false).await.unwrap();
    let asset = scan.checks["quick"].spec.icon_asset.clone().expect("saved");
    assert!(asset.ends_with(".png"));
    assert!(store
        .plugin_icon(&asset)
        .unwrap()
        .starts_with("data:image/png;base64,"));
    let installed = store
        .install_scanned("global", "acme", "quick", false, None)
        .await
        .unwrap();
    assert_eq!(installed.spec.icon_asset.as_deref(), Some(asset.as_str()));
    store.remove_marketplace("acme").unwrap();
    assert!(store.plugin_icon(&asset).is_ok());
    // Names can't reach outside the store, and saving can't claim a logo.
    assert!(store.plugin_icon("../settings.json").is_err());
    assert!(store
        .plugin_icon(&format!("{}.exe", "a".repeat(64)))
        .is_err());
    let saved = store
        .save(
            "global",
            serde_json::from_value(
                json!({"name":"mine","brand":"stripe","icon_asset":asset,"skills":[]}),
            )
            .unwrap(),
            None,
        )
        .unwrap();
    assert!(saved.spec.brand.is_none() && saved.spec.icon_asset.is_none());
}
