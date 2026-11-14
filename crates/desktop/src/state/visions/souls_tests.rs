use std::path::Path;

use super::souls::SOUL_MAX_BYTES;
use super::*;
use crate::state::AppState;

fn state_in(dir: &Path) -> AppState {
    AppState::new_for_test(dir.join("settings.json"))
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

/// A channel led by Juno with Rook working for her; returns (juno, rook).
fn team(state: &AppState, root: &Path) -> (String, String) {
    let juno = state.vision_save(vision("Juno"), None).unwrap().id;
    let rook = state.vision_save(vision("Rook"), None).unwrap().id;
    state
        .channel_save(
            Channel {
                id: String::new(),
                name: "release".into(),
                topic: String::new(),
                project_root: root.to_path_buf(),
                approval_mode: themis_core::configuration::ApprovalMode::Custom,
                members: vec![juno.clone(), rook.clone()],
                chief: juno.clone(),
                autonomy: None,
                custom: None,
                daily_budget_usd: None,
                revision: String::new(),
            },
            None,
        )
        .unwrap();
    (juno, rook)
}

#[test]
fn a_soul_round_trips_and_checks_its_revision() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path());
    let (juno, _) = team(&state, &dir.path().join("p"));
    let empty = state.get_soul(&juno).unwrap();
    assert_eq!(empty.text, "");
    let saved = state
        .save_soul(&juno, "Calm and direct.", &empty.revision)
        .unwrap();
    assert_eq!(state.get_soul(&juno).unwrap().text, "Calm and direct.");
    assert!(state
        .inner
        .visions_dir
        .join("souls")
        .join(format!("{juno}.md"))
        .is_file());
    // Saving from the old copy is stale.
    let stale = state
        .save_soul(&juno, "Loud.", &empty.revision)
        .unwrap_err();
    assert!(stale.contains("changed"), "{stale}");
    // An edit made in an editor is what the next read sees, with a new revision.
    std::fs::write(
        state
            .inner
            .visions_dir
            .join("souls")
            .join(format!("{juno}.md")),
        "Edited by hand.",
    )
    .unwrap();
    let edited = state.get_soul(&juno).unwrap();
    assert_eq!(edited.text, "Edited by hand.");
    assert_ne!(edited.revision, saved.revision);
}

#[test]
fn a_soul_over_16_kib_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path());
    let (juno, _) = team(&state, &dir.path().join("p"));
    let fits = "x".repeat(SOUL_MAX_BYTES);
    let saved = state.save_soul(&juno, &fits, "").unwrap();
    let error = state
        .save_soul(&juno, &"x".repeat(SOUL_MAX_BYTES + 1), &saved.revision)
        .unwrap_err();
    assert!(error.contains("16 KiB"), "{error}");
}

#[test]
fn an_unreadable_soul_is_a_problem_not_silence() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path());
    let (juno, _) = team(&state, &dir.path().join("p"));
    let path = state
        .inner
        .visions_dir
        .join("souls")
        .join(format!("{juno}.md"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
    let view = state.get_soul(&juno).unwrap();
    assert!(view.problem.is_some(), "invalid text is reported");
    assert_eq!(state.soul_for_run(&juno), "");
}

#[test]
fn a_change_applies_from_the_next_wake_not_a_running_one() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path());
    let (juno, _) = team(&state, &dir.path().join("p"));
    state.save_soul(&juno, "First.", "").unwrap();
    // A run builds its head once, at setup, from the soul read then.
    let running = state.soul_for_run(&juno);
    let revision = state.get_soul(&juno).unwrap().revision;
    state.save_soul(&juno, "Second.", &revision).unwrap();
    assert_eq!(running, "First.");
    assert_eq!(state.soul_for_run(&juno), "Second.");
}

#[test]
fn workers_have_no_soul() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path());
    let (_, rook) = team(&state, &dir.path().join("p"));
    let error = state.save_soul(&rook, "I build.", "").unwrap_err();
    assert!(error.contains("chief"), "{error}");
    // Atlas has one.
    state
        .visions_ensure_administration(&dir.path().join("p"))
        .unwrap();
    assert!(state.save_soul(ADMIN_VISION_ID, "Tidy.", "").is_ok());
}
