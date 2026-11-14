use std::path::Path;

use serde_json::{json, Value};

use super::convert::MARKER_FILE;
use super::*;
use crate::state::AppState;

fn state_in(dir: &Path) -> AppState {
    AppState::new_for_test(dir.join("settings.json"))
}

/// A Phase 1 store: `# administration` with the admin, `# docs` with Wren then Rook, Wren with its own
/// autonomy, budget and soul, and a log in `# docs`.
fn phase1_store(visions_dir: &Path, root: &Path) {
    std::fs::create_dir_all(visions_dir.join("channels")).unwrap();
    std::fs::create_dir_all(visions_dir.join("heartbeats")).unwrap();
    std::fs::write(visions_dir.join("heartbeats/wren.md"), "every morning").unwrap();
    let visions = json!([
        {"id": "admin", "name": "Atlas", "role": "Sets up channels and Visions", "shape": "flat",
         "color": "indigo", "revision": "1"},
        {"id": "wren", "name": "Wren", "role": "Writes", "shape": "cloud", "color": "purple",
         "autonomy": "most", "custom": {"delete": true}, "daily_budget_usd": 0.5,
         "soul": "Calm.", "revision": "1"},
        {"id": "rook", "name": "Rook", "role": "Builds", "shape": "cloud", "color": "teal",
         "soul": "Loud.", "revision": "1"}
    ]);
    let channels = json!([
        {"id": "hq1", "name": "hq", "topic": "", "project_root": root, "approval_mode": "custom",
         "members": ["admin"], "revision": "1"},
        {"id": "docs1", "name": "docs", "topic": "Docs", "project_root": root,
         "approval_mode": "custom", "members": ["wren", "rook"], "revision": "1"}
    ]);
    std::fs::write(visions_dir.join("visions.json"), visions.to_string()).unwrap();
    std::fs::write(visions_dir.join("channels.json"), channels.to_string()).unwrap();
    std::fs::write(
        visions_dir.join("channels/docs1.jsonl"),
        "{\"seq\":1,\"at_ms\":1,\"entry\":{\"type\":\"person\",\"text\":\"hello\"}}\n",
    )
    .unwrap();
}

fn notes_in(visions_dir: &Path, channel_id: &str) -> usize {
    std::fs::read_to_string(visions_dir.join(format!("channels/{channel_id}.jsonl")))
        .unwrap_or_default()
        .matches("\"teams_arrived\"")
        .count()
}

fn raw(visions_dir: &Path, file: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(visions_dir.join(file)).unwrap()).unwrap()
}

#[test]
fn phase1_store_converts_first_member_to_chief_and_drops_vision_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects").join("docs");
    let state = state_in(dir.path());
    let visions_dir = state.inner.visions_dir.clone();
    phase1_store(&visions_dir, &root);

    state.visions_ensure_administration(&root).unwrap();

    let channels = state.channel_list(&root);
    let administration = channels.iter().find(|c| c.id == "hq1").unwrap();
    assert_eq!(administration.name, "administration");
    assert_eq!(administration.chief, ADMIN_VISION_ID);
    let docs = channels.iter().find(|c| c.id == "docs1").unwrap();
    assert_eq!(docs.chief, "wren");
    assert_eq!(workers(docs), vec!["rook".to_owned()]);
    let wren = raw(&visions_dir, "visions.json")
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == "wren")
        .unwrap()
        .clone();
    for gone in ["autonomy", "custom", "daily_budget_usd"] {
        assert!(wren.get(gone).is_none(), "{gone} is dropped");
    }
    assert!(!visions_dir.join("heartbeats").exists());
    // Wren leads # docs, so its soul moved to soul.md; Rook works for it and has none.
    assert_eq!(state.get_soul("wren").unwrap().text, "Calm.");
    assert!(wren.get("soul").is_none());
    assert!(!visions_dir.join("souls/rook.md").exists());
    assert!(
        std::fs::read_to_string(visions_dir.join("channels/docs1.jsonl"))
            .unwrap()
            .contains("hello")
    );
    assert_eq!(notes_in(&visions_dir, "hq1"), 1);
    assert!(visions_dir.join(MARKER_FILE).exists());

    // A later start changes nothing and tells nobody again.
    let again = state_in(dir.path());
    again.visions_ensure_administration(&root).unwrap();
    assert_eq!(notes_in(&visions_dir, "hq1"), 1);
}

#[test]
fn an_interrupted_conversion_finishes_once_on_the_next_start() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects").join("docs");
    let state = state_in(dir.path());
    let visions_dir = state.inner.visions_dir.clone();
    phase1_store(&visions_dir, &root);
    // The app quit after the channels were written, before the Visions and the marker.
    super::convert::convert_channels_only_for_test(&visions_dir);
    assert_eq!(
        raw(&visions_dir, "channels.json")[0]["name"],
        "administration"
    );
    assert!(raw(&visions_dir, "visions.json")[1]
        .get("autonomy")
        .is_some());

    state.visions_ensure_administration(&root).unwrap();
    assert!(raw(&visions_dir, "visions.json")
        .as_array()
        .unwrap()
        .iter()
        .all(|v| v.get("autonomy").is_none()));
    assert_eq!(notes_in(&visions_dir, "hq1"), 1);

    // Quit again after the note but before the marker: the next start posts no second note.
    std::fs::remove_file(visions_dir.join(MARKER_FILE)).unwrap();
    let again = state_in(dir.path());
    again.visions_ensure_administration(&root).unwrap();
    assert_eq!(notes_in(&visions_dir, "hq1"), 1);
    assert!(visions_dir.join(MARKER_FILE).exists());
}

fn vision(name: &str) -> Vision {
    Vision {
        id: String::new(),
        name: name.into(),
        role: String::new(),
        shape: "cloud".into(),
        color: "blue".into(),
        accessory: "none".into(),
        model: None,
        paused: false,
        revision: String::new(),
    }
}

fn channel(name: &str, root: &Path, members: Vec<String>, chief: &str) -> Channel {
    Channel {
        id: String::new(),
        name: name.into(),
        topic: String::new(),
        project_root: root.to_path_buf(),
        approval_mode: themis_core::configuration::ApprovalMode::Custom,
        members,
        chief: chief.into(),
        autonomy: None,
        custom: None,
        daily_budget_usd: None,
        revision: String::new(),
    }
}

#[test]
fn a_channel_names_its_chief_among_its_members() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects").join("docs");
    let state = state_in(dir.path());
    let juno = state.vision_save(vision("Juno"), None).unwrap().id;
    let rook = state.vision_save(vision("Rook"), None).unwrap().id;
    let outsider = state.vision_save(vision("Pip"), None).unwrap().id;
    let team = vec![juno.clone(), rook.clone()];

    let saved = state
        .channel_save(channel("release", &root, team.clone(), &juno), None)
        .unwrap();
    assert!(saved.is_chief(&juno));
    assert_eq!(workers(&saved), vec![rook.clone()]);
    let error = state
        .channel_save(channel("other", &root, team.clone(), &outsider), None)
        .unwrap_err();
    assert!(error.contains("chief"), "{error}");
    // No chief named: the first member leads.
    let defaulted = state
        .channel_save(channel("third", &root, team, ""), None)
        .unwrap();
    assert_eq!(defaulted.chief, juno);
    // A Vision's accessory is one of the cast's.
    let mut odd = vision("Odd");
    odd.accessory = "cape".into();
    assert!(state.vision_save(odd, None).is_err());
}

#[test]
fn the_admin_is_chief_only_of_administration() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects").join("docs");
    let state = state_in(dir.path());
    state.visions_ensure_administration(&root).unwrap();
    let administration = state
        .channel_list(&root)
        .into_iter()
        .find(|c| c.name == "administration")
        .unwrap();
    assert_eq!(administration.chief, ADMIN_VISION_ID);
    assert_eq!(workers(&administration), Vec::<String>::new());
    let juno = state.vision_save(vision("Juno"), None).unwrap().id;
    let admin_elsewhere = channel(
        "release",
        &root,
        vec![ADMIN_VISION_ID.to_owned(), juno.clone()],
        ADMIN_VISION_ID,
    );
    assert!(state.channel_save(admin_elsewhere, None).is_err());
    let mut with_worker = administration.clone();
    with_worker.members.push(juno);
    assert!(state
        .channel_save(with_worker, Some(&administration.revision))
        .is_err());
}

/// A Phase 1 project that already had a channel named `administration`, a channel with no
/// members, owned chats, and Visions stricter than Settings.
fn crowded_phase1_store(visions_dir: &Path, root: &Path) {
    phase1_store(visions_dir, root);
    let mut channels = raw(visions_dir, "channels.json");
    channels.as_array_mut().unwrap().extend([
        json!({"id": "adm1", "name": "administration", "topic": "Office", "project_root": root,
               "approval_mode": "custom", "members": ["rook"], "revision": "1"}),
        json!({"id": "empty1", "name": "empty", "topic": "", "project_root": root,
               "approval_mode": "custom", "members": [], "revision": "1"}),
    ]);
    std::fs::write(visions_dir.join("channels.json"), channels.to_string()).unwrap();
    let mut visions = raw(visions_dir, "visions.json");
    // Rook (in # docs with Wren) asks about everything and spends little.
    let rook = visions
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|v| v["id"] == "rook")
        .unwrap();
    rook["autonomy"] = json!("ask");
    rook["daily_budget_usd"] = json!(0.2);
    std::fs::write(visions_dir.join("visions.json"), visions.to_string()).unwrap();
    std::fs::write(
        visions_dir.join("owners.json"),
        r#"[{"thread_id":"t1","vision_id":"wren","channel_id":"docs1","kind":"work","hold":"none"}]"#,
    )
    .unwrap();
}

#[test]
fn a_channel_already_named_administration_steps_aside() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects").join("docs");
    let state = state_in(dir.path());
    let visions_dir = state.inner.visions_dir.clone();
    crowded_phase1_store(&visions_dir, &root);

    state.visions_ensure_administration(&root).unwrap();

    let channels = state.channel_list(&root);
    let named: Vec<&str> = channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        named
            .iter()
            .filter(|name| **name == "administration")
            .count(),
        1
    );
    assert_eq!(
        channels.iter().find(|c| c.id == "hq1").unwrap().name,
        "administration"
    );
    let theirs = channels.iter().find(|c| c.id == "adm1").unwrap();
    assert_eq!(theirs.name, "administration-team");
    assert_eq!(theirs.chief, "rook");
    // The built-in channel is listed first.
    assert_eq!(channels[0].id, "hq1");
    // A channel with no members has no chief yet.
    assert_eq!(
        channels.iter().find(|c| c.id == "empty1").unwrap().chief,
        ""
    );
    assert_eq!(notes_in(&visions_dir, "hq1"), 1);
    assert_eq!(notes_in(&visions_dir, "adm1"), 0);
}

#[test]
fn the_strictest_member_limits_carry_over_to_the_channel() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects").join("docs");
    let state = state_in(dir.path());
    let visions_dir = state.inner.visions_dir.clone();
    crowded_phase1_store(&visions_dir, &root);

    state.visions_ensure_administration(&root).unwrap();

    // # docs had Wren (most, $0.5) and Rook (ask, $0.2): the team asks and spends $0.2.
    let docs = state
        .channel_list(&root)
        .into_iter()
        .find(|c| c.id == "docs1")
        .unwrap();
    assert_eq!(docs.autonomy, Some(themis_core::visions::Autonomy::Ask));
    assert_eq!(docs.daily_budget_usd, Some(0.2));
}

#[test]
fn owned_chats_are_left_exactly_as_they_were() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects").join("docs");
    let state = state_in(dir.path());
    let visions_dir = state.inner.visions_dir.clone();
    crowded_phase1_store(&visions_dir, &root);
    let before = std::fs::read(visions_dir.join("owners.json")).unwrap();

    state.visions_ensure_administration(&root).unwrap();

    assert_eq!(
        std::fs::read(visions_dir.join("owners.json")).unwrap(),
        before
    );
}

#[test]
fn a_damaged_visions_file_converts_nothing_until_it_is_fixed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects").join("docs");
    let visions_dir = state_in(dir.path()).inner.visions_dir.clone();
    phase1_store(&visions_dir, &root);
    let good = std::fs::read(visions_dir.join("visions.json")).unwrap();
    std::fs::write(visions_dir.join("visions.json"), "not json").unwrap();

    let state = state_in(dir.path());
    assert!(state.visions_ensure_administration(&root).is_err());
    assert!(!visions_dir.join(MARKER_FILE).exists());
    assert_eq!(
        raw(&visions_dir, "channels.json")[0]["name"],
        "hq",
        "nothing half-moved"
    );

    std::fs::write(visions_dir.join("visions.json"), good).unwrap();
    let fixed = state_in(dir.path());
    fixed.visions_ensure_administration(&root).unwrap();
    assert!(visions_dir.join(MARKER_FILE).exists());
    assert_eq!(notes_in(&visions_dir, "hq1"), 1);
}
