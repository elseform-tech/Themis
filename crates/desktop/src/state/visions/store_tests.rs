use std::path::{Path, PathBuf};

use themis_core::configuration::ApprovalMode;

use super::*;
use crate::state::AppState;

fn state_in(dir: &Path) -> AppState {
    AppState::new_for_test(dir.join("settings.json"))
}

fn vision(name: &str) -> Vision {
    Vision {
        id: String::new(),
        name: name.into(),
        role: "Keeps the docs tidy".into(),
        shape: "cloud".into(),
        color: "blue".into(),
        model: None,
        autonomy: None,
        custom: None,
        daily_budget_usd: None,
        soul: String::new(),
        paused: false,
        revision: String::new(),
    }
}

fn channel(name: &str, root: &Path, members: Vec<String>) -> Channel {
    Channel {
        id: String::new(),
        name: name.into(),
        topic: "Docs".into(),
        project_root: root.to_path_buf(),
        approval_mode: ApprovalMode::Custom,
        members,
        revision: String::new(),
    }
}

fn project(dir: &Path, name: &str) -> PathBuf {
    dir.join("projects").join(name)
}

#[test]
fn saving_with_a_stale_revision_fails() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path());
    let saved = state.vision_save(vision("Wren"), None).unwrap();
    assert!(!saved.id.is_empty());
    assert!(!saved.revision.is_empty());

    let mut renamed = saved.clone();
    renamed.name = "Wren the second".into();
    let newer = state
        .vision_save(renamed.clone(), Some(&saved.revision))
        .unwrap();
    assert_ne!(newer.revision, saved.revision);

    // Saving again from the first copy is stale.
    let error = state
        .vision_save(renamed.clone(), Some(&saved.revision))
        .unwrap_err();
    assert_eq!(
        error,
        "Vision changed since it was opened; reload before saving"
    );
    // So is saving an existing Vision as if it were new.
    assert!(state.vision_save(renamed, None).is_err());
    assert_eq!(state.visions_list(), vec![newer]);

    // Channels check their revision the same way.
    let root = project(dir.path(), "docs");
    let first = state
        .channel_save(channel("docs", &root, Vec::new()), None)
        .unwrap();
    let second = state
        .channel_save(first.clone(), Some(&first.revision))
        .unwrap();
    assert!(state
        .channel_save(first.clone(), Some(&first.revision))
        .is_err());
    assert_eq!(state.channel_list(&root), vec![second]);
}

#[test]
fn vision_fields_are_validated() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path());
    let mut long_soul = vision("Wren");
    long_soul.soul = "x".repeat(SOUL_MAX_BYTES + 1);
    assert!(state.vision_save(long_soul, None).is_err());
    let mut fits = vision("Wren");
    fits.soul = "x".repeat(SOUL_MAX_BYTES);
    assert!(state.vision_save(fits, None).is_ok());
    assert!(state.vision_save(vision("  "), None).is_err());
    let mut spends = vision("Wren");
    spends.custom = Some([("spend".to_string(), true)].into());
    assert!(state.vision_save(spends, None).is_err());
    let mut unknown_row = vision("Wren");
    unknown_row.custom = Some([("everything".to_string(), true)].into());
    assert!(state.vision_save(unknown_row, None).is_err());
    let mut negative = vision("Wren");
    negative.daily_budget_usd = Some(-1.0);
    assert!(state.vision_save(negative, None).is_err());
    let mut unknown_id = vision("Wren");
    unknown_id.id = "not-saved-yet".into();
    assert!(state.vision_save(unknown_id, None).is_err());
}

#[test]
fn channel_names_are_unique_per_project_and_validated() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path());
    let docs = project(dir.path(), "docs");
    let site = project(dir.path(), "site");
    let wren = state.vision_save(vision("Wren"), None).unwrap();

    let general = state
        .channel_save(channel("general", &docs, vec![wren.id.clone()]), None)
        .unwrap();
    assert_eq!(general.members, vec![wren.id.clone()]);
    // The same name in the same project is refused; in another project it is fine.
    assert!(state
        .channel_save(channel("general", &docs, Vec::new()), None)
        .is_err());
    state
        .channel_save(channel("general", &site, Vec::new()), None)
        .unwrap();
    // A channel keeps its own name when saved again.
    state
        .channel_save(general.clone(), Some(&general.revision))
        .unwrap();

    for bad in ["", "General", "has space", "under_score", &"a".repeat(41)] {
        assert!(
            state
                .channel_save(channel(bad, &docs, Vec::new()), None)
                .is_err(),
            "{bad:?} was accepted"
        );
    }
    assert!(state
        .channel_save(channel(&"a".repeat(40), &docs, Vec::new()), None)
        .is_ok());
    // Members must be saved Visions.
    assert!(state
        .channel_save(channel("ghosts", &docs, vec!["nobody".into()]), None)
        .is_err());
    // A relative project folder is refused.
    assert!(state
        .channel_save(channel("relative", Path::new("docs"), Vec::new()), None)
        .is_err());

    let names: Vec<String> = state
        .channel_list(&docs)
        .into_iter()
        .map(|channel| channel.name)
        .collect();
    assert_eq!(names, vec!["a".repeat(40), "general".to_string()]);
}

#[test]
fn owners_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Owner {
        thread_id: "thread-1".into(),
        vision_id: "vision-1".into(),
        channel_id: "channel-1".into(),
        kind: OwnedKind::Work,
        hold: Hold::TakenOver,
    };
    {
        let state = state_in(dir.path());
        state.owner_set(owner.clone()).unwrap();
        state
            .owner_set(Owner {
                thread_id: "thread-2".into(),
                kind: OwnedKind::VisionChat,
                hold: Hold::None,
                ..owner.clone()
            })
            .unwrap();
        state.owner_release("thread-2").unwrap();
        assert!(state
            .owner_set(Owner {
                thread_id: "../escape".into(),
                ..owner.clone()
            })
            .is_err());
    }
    let state = state_in(dir.path());
    assert_eq!(state.owner_of("thread-1"), Some(owner));
    assert_eq!(state.owner_of("thread-2"), None);
}

#[test]
fn a_damaged_store_file_is_reported_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join(VISIONS_DIR);
    std::fs::create_dir_all(&store).unwrap();
    let damaged = store.join(CHANNELS_FILE);
    std::fs::write(&damaged, b"{ not json").unwrap();

    assert!(VisionsCache::default().channels(&store).is_err());

    let state = state_in(dir.path());
    let root = project(dir.path(), "docs");
    let error = state
        .channel_save(channel("docs", &root, Vec::new()), None)
        .unwrap_err();
    assert!(error.contains(CHANNELS_FILE), "{error}");
    assert!(state.channel_list(&root).is_empty());
    assert!(state
        .channel_append("anything", ChannelEntry::Person { text: "hi".into() })
        .is_err());
    assert_eq!(std::fs::read(&damaged).unwrap(), b"{ not json");

    // The other files keep working.
    state.vision_save(vision("Wren"), None).unwrap();
    assert_eq!(state.visions_list().len(), 1);
}

#[cfg(unix)]
#[test]
fn a_symlinked_store_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join(VISIONS_DIR);
    std::fs::create_dir_all(&store).unwrap();
    let elsewhere = dir.path().join("elsewhere.json");
    std::fs::write(&elsewhere, b"[]").unwrap();
    std::os::unix::fs::symlink(&elsewhere, store.join(VISIONS_FILE)).unwrap();
    let state = state_in(dir.path());
    assert!(state.vision_save(vision("Wren"), None).is_err());
    assert_eq!(std::fs::read(&elsewhere).unwrap(), b"[]");
}

#[test]
fn a_store_file_edited_on_disk_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path());
    let saved = state.vision_save(vision("Wren"), None).unwrap();
    let path = dir.path().join(VISIONS_DIR).join(VISIONS_FILE);
    // A hand edit: still valid, different bytes.
    let mut edited = std::fs::read(&path).unwrap();
    edited.extend_from_slice(b"\n\n");
    std::fs::write(&path, &edited).unwrap();

    let error = state
        .vision_save(saved.clone(), Some(&saved.revision))
        .unwrap_err();
    assert!(error.contains("changed on disk"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), edited);

    // The next call reads the edited file, and saving from it works.
    let reloaded = state.visions_list();
    assert_eq!(reloaded, vec![saved.clone()]);
    state
        .vision_save(reloaded[0].clone(), Some(&reloaded[0].revision))
        .unwrap();
}
