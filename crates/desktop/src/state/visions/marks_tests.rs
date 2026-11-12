use super::*;
use crate::state::AppState;
use crate::types::ProviderKind;

/// A state with one Vision and `size` chats in one project: every other chat a Vision's work
/// chat, every fourth its own chat. Returns the state, the project and the work chats.
async fn chats(size: usize) -> (tempfile::TempDir, AppState, String, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let project = project.to_string_lossy().into_owned();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let vision = Vision {
        id: String::new(),
        name: "Wren".into(),
        role: String::new(),
        shape: "cloud".into(),
        color: "teal".into(),
        model: None,
        autonomy: None,
        custom: None,
        daily_budget_usd: None,
        soul: String::new(),
        paused: false,
        revision: String::new(),
    };
    let vision = state.vision_save(vision, None).unwrap();
    let mut work = Vec::new();
    for at in 0..size {
        let info = state
            .create_thread(project.clone(), ProviderKind::Go, Some("m".into()))
            .await
            .unwrap();
        let kind = match at % 4 {
            1 | 3 => OwnedKind::Work,
            2 => OwnedKind::VisionChat,
            _ => continue,
        };
        if kind == OwnedKind::Work {
            work.push(info.id.clone());
        }
        let owner = Owner {
            thread_id: info.id,
            vision_id: vision.id.clone(),
            channel_id: "c".into(),
            kind,
            hold: Hold::None,
        };
        state.owner_set(owner).unwrap();
    }
    (dir, state, project, work)
}

/// Scale: plain chat's listing reads who runs which chat once, however many chats Visions
/// own (20 and 200 chats: one snapshot each), and marks each work chat from it.
#[tokio::test]
async fn a_listing_reads_the_owners_once_however_many_chats() {
    let mut snapshots = Vec::new();
    for size in [20, 200] {
        let (_dir, state, project, work) = chats(size).await;
        state.visions_cache().marks_built = 0;
        let listed = state.list_threads(project).await.unwrap();
        snapshots.push(state.visions_cache().marks_built);
        assert_eq!(
            listed.len(),
            size - size / 4,
            "a Vision's own chats stay hidden"
        );
        let marked: Vec<&str> = listed
            .iter()
            .filter(|info| info.vision.as_ref().is_some_and(|v| v.name == "Wren"))
            .map(|info| info.id.as_str())
            .collect();
        assert_eq!(marked.len(), work.len());
        assert!(work.iter().all(|id| marked.contains(&id.as_str())));
    }
    assert_eq!(snapshots, vec![1, 1]);
}
