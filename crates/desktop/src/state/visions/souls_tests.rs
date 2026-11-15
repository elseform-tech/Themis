//! A soul is text on a chief's or Atlas's record, saved from the profile; workers have none.

use std::path::Path;

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
        soul: String::new(),
        color: "blue".into(),
        accessory: "none".into(),
        model: None,
        paused: false,
        revision: String::new(),
    }
}

fn release(root: &Path, members: Vec<String>) -> Channel {
    Channel {
        id: String::new(),
        name: "release".into(),
        topic: String::new(),
        project_root: root.to_path_buf(),
        full_access: false,
        chief: members[0].clone(),
        members,
        autonomy: None,
        custom: None,
        daily_budget_usd: None,
        paused_workers: Vec::new(),
        revision: String::new(),
    }
}

/// A channel led by Juno with Rook working for her; returns (juno, rook).
fn team(state: &AppState, root: &Path) -> (Vision, Vision) {
    let juno = state.vision_save(vision("Juno"), None).unwrap();
    let rook = state.vision_save(vision("Rook"), None).unwrap();
    state
        .channel_save(release(root, vec![juno.id.clone(), rook.id.clone()]), None)
        .unwrap();
    (juno, rook)
}

#[test]
fn a_chief_saves_its_soul_from_the_profile() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path());
    let (juno, _) = team(&state, &dir.path().join("p"));
    let revision = juno.revision.clone();
    let saved = state
        .vision_save(
            Vision {
                soul: "Calm and direct.".into(),
                ..juno
            },
            Some(&revision),
        )
        .unwrap();
    assert_eq!(
        state.vision_get(&saved.id).unwrap().soul,
        "Calm and direct."
    );
}

#[test]
fn a_soul_over_16_kib_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path());
    let (juno, _) = team(&state, &dir.path().join("p"));
    let revision = juno.revision.clone();
    let fits = state
        .vision_save(
            Vision {
                soul: "x".repeat(16 * 1024),
                ..juno
            },
            Some(&revision),
        )
        .unwrap();
    let revision = fits.revision.clone();
    let error = state
        .vision_save(
            Vision {
                soul: "x".repeat(16 * 1024 + 1),
                ..fits
            },
            Some(&revision),
        )
        .unwrap_err();
    assert!(error.contains("16 KiB"), "{error}");
}

#[test]
fn workers_have_no_soul() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path());
    let root = dir.path().join("p");
    let (_, rook) = team(&state, &root);
    let revision = rook.revision.clone();
    let error = state
        .vision_save(
            Vision {
                soul: "I build.".into(),
                ..rook.clone()
            },
            Some(&revision),
        )
        .unwrap_err();
    assert!(error.contains("chief"), "{error}");
    // Other edits to a worker still save.
    assert!(state
        .vision_save(
            Vision {
                role: "Builds".into(),
                ..rook
            },
            Some(&revision)
        )
        .is_ok());
    // Atlas has one.
    state.visions_ensure_administration(&root).unwrap();
    let atlas = state.vision_get(ADMIN_VISION_ID).unwrap();
    let revision = atlas.revision.clone();
    assert!(state
        .vision_save(
            Vision {
                soul: "Tidy.".into(),
                ..atlas
            },
            Some(&revision)
        )
        .is_ok());
}

#[test]
fn a_former_chief_keeps_its_soul_but_works_without_it() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path());
    let root = dir.path().join("p");
    let (juno, rook) = team(&state, &root);
    let revision = juno.revision.clone();
    let juno = state
        .vision_save(
            Vision {
                soul: "Calm and direct.".into(),
                ..juno
            },
            Some(&revision),
        )
        .unwrap();
    let mut channel = state.channel_list(&root).into_iter().next().unwrap();
    let led = runs::vision_head(&juno, &channel, &Default::default(), &[]);
    assert!(led.contains("Calm and direct."));
    // Rook leads now: Juno's soul stays on her record, and her runs go without it.
    channel.chief = rook.id.clone();
    let revision = channel.revision.clone();
    let channel = state.channel_save(channel, Some(&revision)).unwrap();
    let juno = state.vision_get(&juno.id).unwrap();
    assert_eq!(juno.soul, "Calm and direct.");
    assert!(
        !runs::vision_head(&juno, &channel, &Default::default(), &[]).contains("Calm and direct.")
    );
    // Saving her profile with the soul she already has is not a new soul.
    let revision = juno.revision.clone();
    assert!(state
        .vision_save(
            Vision {
                role: "Writes".into(),
                ..juno
            },
            Some(&revision)
        )
        .is_ok());
}
