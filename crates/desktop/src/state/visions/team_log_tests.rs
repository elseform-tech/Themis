use std::path::Path;

use super::team_log::TeamEntry;
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
        shape: "cloud".into(),
        color: "blue".into(),
        accessory: "none".into(),
        model: None,
        paused: false,
        revision: String::new(),
    }
}

/// `# release`, led by Juno with Rook working; returns (channel, juno, rook).
fn team(state: &AppState, root: &Path) -> (String, String, String) {
    let juno = state.vision_save(vision("Juno"), None).unwrap().id;
    let rook = state.vision_save(vision("Rook"), None).unwrap().id;
    let channel = state
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
                paused_workers: Vec::new(),
                revision: String::new(),
            },
            None,
        )
        .unwrap();
    (channel.id, juno, rook)
}

#[test]
fn a_team_log_keeps_tasks_and_reports_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path());
    let (channel, juno, rook) = team(&state, &dir.path().join("p"));
    let task = TeamEntry::Task {
        from_chief: juno.clone(),
        text: "Fix the build".into(),
    };
    assert_eq!(state.team_append(&channel, &rook, task.clone()).unwrap(), 1);
    let report = TeamEntry::Report {
        text: "Build is green".into(),
    };
    assert_eq!(
        state.team_append(&channel, &rook, report.clone()).unwrap(),
        2
    );
    let page = state.team_page(&channel, &rook, None, 64 * 1024).unwrap();
    let entries: Vec<TeamEntry> = page.entries.into_iter().map(|line| line.entry).collect();
    assert_eq!(entries, vec![task, report]);
    assert_eq!(page.next_before, None);
    // A fresh state reads on from the file.
    let again = state_in(dir.path());
    assert_eq!(
        again
            .team_append(&channel, &rook, TeamEntry::Paused)
            .unwrap(),
        3
    );
}

#[test]
fn only_a_channels_workers_have_a_team_log() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path());
    let (channel, juno, _) = team(&state, &dir.path().join("p"));
    let note = || TeamEntry::Report { text: "x".into() };
    // The chief has none, nor does a stranger, nor an unsafe id.
    assert!(state.team_append(&channel, &juno, note()).is_err());
    assert!(state.team_append(&channel, "stranger", note()).is_err());
    assert!(state.team_append(&channel, "../escape", note()).is_err());
    assert!(state.team_append("../c", "x", note()).is_err());
}
