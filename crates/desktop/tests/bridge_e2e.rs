//! Headless bridge tests: the real [`AppState`](themis_desktop::state::AppState)
//! handlers driven with a [`ChannelSink`](themis_desktop::sink::ChannelSink),
//! a temp git repo, and a scripted mock LLM (wiremock). No network beyond
//! localhost; tests needing `git` skip gracefully when the binary is missing.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

mod common;
use common::{git, git_available, init_repo, mount_scripted_llm};

use themis_desktop::sink::{ChannelSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{ApprovalDecision, ProviderKind, ThreadEvent};
use tokio::sync::mpsc::UnboundedReceiver;

/// Overall deadline for an LLM-driven run (fail fast instead of hanging on
/// the 300s approval timeout).
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

fn test_state() -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    (state, dir)
}

struct ScriptedRun {
    events: Vec<ThreadEvent>,
    approvals_answered: usize,
}

/// Sends one message and drives it to a terminal event, answering every
/// approval dialog with `AllowOnce`. Returns the collected thread events.
async fn drive_run(
    state: &AppState,
    rx: &mut UnboundedReceiver<TestEvent>,
    thread_id: &str,
    expected_run: &str,
) -> ScriptedRun {
    let mut events = Vec::new();
    let mut approvals_answered = 0;
    let outcome = tokio::time::timeout(RUN_TIMEOUT, async {
        while let Some(event) = rx.recv().await {
            match event {
                TestEvent::Approval(request) => {
                    assert_eq!(request.thread_id, thread_id);
                    state
                        .approve_action(
                            thread_id.to_owned(),
                            request.approval_id.clone(),
                            ApprovalDecision::Once,
                        )
                        .await
                        .expect("approve");
                    approvals_answered += 1;
                }
                TestEvent::Thread(envelope) => {
                    assert_eq!(envelope.thread_id, thread_id);
                    assert_eq!(envelope.run_id, expected_run);
                    let terminal = matches!(
                        envelope.event,
                        ThreadEvent::Finished { .. } | ThreadEvent::Failed { .. }
                    );
                    events.push(envelope.event);
                    if terminal {
                        break;
                    }
                }
                TestEvent::Review(item) => {
                    panic!("unexpected review item in a non-automation run: {item:?}");
                }
            }
        }
    })
    .await;
    assert!(
        outcome.is_ok(),
        "run finished before the {RUN_TIMEOUT:?} deadline"
    );
    ScriptedRun {
        events,
        approvals_answered,
    }
}

fn event_kinds(events: &[ThreadEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            ThreadEvent::Started { .. } => "started",
            ThreadEvent::AssistantText { .. } => "assistant_text",
            ThreadEvent::ToolStarted { .. } => "tool_started",
            ThreadEvent::ToolFinished { .. } => "tool_finished",
            ThreadEvent::ApprovalDecided { .. } => "approval_decided",
            ThreadEvent::Finished { .. } => "finished",
            ThreadEvent::Failed { .. } => "failed",
            ThreadEvent::ContextCheckpoint { .. } => "context_checkpoint",
            ThreadEvent::ContextCompacting => "context_compacting",
            ThreadEvent::Incomplete { .. } => "incomplete",
        })
        .collect()
}

#[tokio::test]
async fn scripted_write_file_run_finishes_and_shows_in_diff() {
    if !git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    let (_repo_temp, root) = init_repo();
    let (state, _settings_temp) = test_state();
    let server = mount_scripted_llm().await;

    let project = state
        .open_project(root.to_string_lossy().into_owned())
        .await
        .expect("open project");
    assert!(project.is_git);

    state
        .set_secret("go".to_owned(), "test-key".to_owned())
        .await
        .expect("store key");
    state.set_go_base_url_override(Some(server.uri()));

    let thread = state
        .create_thread(
            root.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".to_owned()),
        )
        .await
        .expect("create thread");

    let (sink, mut rx) = ChannelSink::channel();
    let handle = state
        .send_message(
            Arc::new(sink),
            thread.id.clone(),
            "write hello.txt".to_owned(),
        )
        .await
        .expect("send");
    let run = drive_run(&state, &mut rx, &thread.id, &handle.run_id).await;

    // Exactly one dialog: dispatch consults the hook and the execution-time
    // check consumes the dispatch permit instead of prompting again.
    assert_eq!(run.approvals_answered, 1);
    let kinds = event_kinds(&run.events);
    // The final turn emits its text as `assistant_text`, then `finished`.
    assert_eq!(
        kinds,
        vec![
            "started",
            "tool_started",
            "approval_decided",
            "tool_finished",
            "assistant_text",
            "finished",
        ],
        "{kinds:?}"
    );
    let ThreadEvent::ToolStarted { tool, .. } = &run.events[1] else {
        panic!("expected tool_started, got {:?}", run.events[1]);
    };
    assert_eq!(tool, "write_file");
    let ThreadEvent::ApprovalDecided { tool, decision } = &run.events[2] else {
        panic!("expected approval_decided, got {:?}", run.events[2]);
    };
    assert_eq!(tool, "write_file");
    assert_eq!(*decision, ApprovalDecision::Once);
    let ThreadEvent::ToolFinished { tool, ok, output } = &run.events[3] else {
        panic!("expected tool_finished, got {:?}", run.events[3]);
    };
    assert_eq!(tool, "write_file");
    assert!(ok, "tool call succeeded");
    assert!(
        !output.is_empty(),
        "tool output reaches the conversation bridge"
    );
    let ThreadEvent::Finished { result } = run.events.last().expect("terminal event") else {
        panic!("expected finished, got {:?}", run.events.last());
    };
    assert_eq!(result, "done writing the file");

    // The agent writes directly into the shared project checkout.
    assert!(thread.worktree_path.is_none());
    assert!(thread.branch.is_none());
    assert_eq!(
        std::fs::read_to_string(root.join("hello.txt")).unwrap(),
        "hi from agent\n"
    );
    // ... and the diff lists it as a fresh (non-preexisting) addition.
    let diff = state.list_diff(thread.id.clone()).await.expect("diff");
    assert!(diff.available);
    assert!(diff.reason.is_none());
    let entry = diff
        .files
        .iter()
        .find(|file| file.path == "hello.txt")
        .expect("hello.txt in diff");
    assert!(!entry.preexisting);
    assert_eq!(entry.hunks.len(), 1);

    // The run cleared the busy flag and titled the thread from the message.
    let info = state.get_thread(&thread.id).await.expect("info");
    assert!(!info.running);
    assert_eq!(info.title, "write hello.txt");
}

#[tokio::test]
async fn busy_thread_rejects_second_send_and_provider_switch() {
    if !git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    let (_repo_temp, root) = init_repo();
    let (state, _settings_temp) = test_state();
    let server = mount_scripted_llm().await;
    state
        .set_secret("go".to_owned(), "test-key".to_owned())
        .await
        .expect("store key");
    state.set_go_base_url_override(Some(server.uri()));
    let thread = state
        .create_thread(
            root.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".to_owned()),
        )
        .await
        .expect("create thread");

    let (sink, mut rx) = ChannelSink::channel();
    let handle = state
        .send_message(
            Arc::new(sink),
            thread.id.clone(),
            "write hello.txt".to_owned(),
        )
        .await
        .expect("first send spawns");

    // While the first run is alive (it will block on its approval dialog)...
    let (sink, _rx) = ChannelSink::channel();
    let busy = state
        .send_message(Arc::new(sink), thread.id.clone(), "second".to_owned())
        .await
        .expect_err("busy");
    assert!(busy.contains("busy"), "{busy}");
    let switched = state
        .set_provider(thread.id.clone(), ProviderKind::Go, None)
        .await
        .expect_err("busy switch");
    assert!(switched.contains("busy"), "{switched}");

    // ... and answering the dialog lets the run finish normally.
    let run = drive_run(&state, &mut rx, &thread.id, &handle.run_id).await;
    assert!(matches!(
        run.events.last(),
        Some(ThreadEvent::Finished { .. })
    ));
}

#[tokio::test]
async fn diff_lifecycle_preserves_preexisting_shared_changes() {
    if !git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    let (_repo_temp, root) = init_repo();
    std::fs::write(root.join("tracked.txt"), "v1\n").expect("write");
    std::fs::write(root.join("other.txt"), "o1\n").expect("write");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "two files"]);

    // Checkout dirt made before the thread starts stays visible and protected.
    std::fs::write(root.join("tracked.txt"), "checkout-dirt\n").expect("write");

    let (state, _settings_temp) = test_state();
    let thread = state
        .create_thread(
            root.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("model".to_owned()),
        )
        .await
        .expect("create thread");
    let worktree = root.clone();

    // Existing checkout changes are visible to the new thread.
    let diff = state.list_diff(thread.id.clone()).await.expect("diff");
    assert!(diff.available);
    assert!(diff
        .files
        .iter()
        .any(|file| file.path == "tracked.txt" && file.preexisting));

    // New shared-workspace changes: a modification plus an
    // untracked file.
    std::fs::write(worktree.join("other.txt"), "o2\n").expect("write");
    std::fs::write(worktree.join("added.txt"), "new\n").expect("write");

    let diff = state.list_diff(thread.id.clone()).await.expect("diff");
    assert!(diff.available);
    let by_path = |name: &str| {
        diff.files
            .iter()
            .find(|file| file.path == name)
            .unwrap_or_else(|| panic!("{name} in diff: {:?}", diff.files))
    };
    assert!(!by_path("other.txt").preexisting);
    assert!(!by_path("added.txt").preexisting);
    assert!(by_path("tracked.txt").preexisting);
    assert!(state
        .discard_file(thread.id.clone(), "tracked.txt".into())
        .await
        .is_err());

    // Thread-made changes discard cleanly, restoring worktree files to HEAD.
    state
        .discard_file(thread.id.clone(), "other.txt".to_owned())
        .await
        .expect("discard tracked");
    assert_eq!(
        std::fs::read_to_string(worktree.join("other.txt")).expect("read"),
        "o1\n"
    );
    state
        .discard_file(thread.id.clone(), "added.txt".to_owned())
        .await
        .expect("discard added");
    assert!(!worktree.join("added.txt").exists());
    // The preexisting change was preserved.
    assert_eq!(
        std::fs::read_to_string(root.join("tracked.txt")).expect("read"),
        "checkout-dirt\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("other.txt")).expect("read"),
        "o1\n"
    );

    // Acceptance records and comments round-trip.
    state
        .accept_file(thread.id.clone(), "other.txt".to_owned())
        .await
        .expect("accept");
    let comment = state
        .add_comment(
            thread.id.clone(),
            "other.txt".to_owned(),
            "ship it".to_owned(),
        )
        .await
        .expect("comment");
    assert_eq!(comment.path, "other.txt");
    assert_eq!(comment.comment, "ship it");
}

#[tokio::test]
async fn non_git_project_has_no_diff_and_read_only_runs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("plain");
    std::fs::create_dir(&root).expect("mkdir");
    let (state, _settings_temp) = test_state();
    let project = state
        .open_project(root.to_string_lossy().into_owned())
        .await
        .expect("open");
    assert!(!project.is_git);
    let thread = state
        .create_thread(root.to_string_lossy().into_owned(), ProviderKind::Go, None)
        .await
        .expect("create");
    assert!(thread.worktree_path.is_none());
    assert!(thread.branch.is_none());
    assert!(thread.base_branch.is_none());
    assert!(!thread.recovered);
    let diff = state.list_diff(thread.id.clone()).await.expect("diff");
    assert!(!diff.available);
    assert!(diff.reason.is_some());
    // Discards need git too.
    let err = state
        .discard_file(thread.id.clone(), "x.txt".to_owned())
        .await
        .expect_err("no git");
    assert!(err.contains("git"), "{err}");
    // Provider switching works while idle.
    let info = state
        .set_provider(
            thread.id.clone(),
            ProviderKind::Go,
            Some("gpt-x".to_owned()),
        )
        .await
        .expect("switch");
    assert_eq!(info.provider, ProviderKind::Go);
    assert_eq!(info.model, "gpt-x");
}

#[tokio::test]
async fn name_only_project_creation_is_safe_and_ready_for_a_thread() {
    let (state, directory) = test_state();
    let projects = directory.path().join("projects");
    state
        .update_settings(themis_desktop::types::SettingsPatch {
            projects_directory: Some(projects.to_string_lossy().into_owned()),
            ..Default::default()
        })
        .await
        .unwrap();
    for name in ["", "../escape", "a/b", "a\\b", ".hidden", "bad:name"] {
        assert!(
            state.create_project(name.to_owned(), None).await.is_err(),
            "{name}"
        );
    }
    let project = state
        .create_project("Clear project".to_owned(), None)
        .await
        .unwrap();
    assert!(project.is_git);
    assert_eq!(
        Path::new(&project.root).parent(),
        Some(projects.canonicalize().unwrap().as_path())
    );
    std::fs::write(Path::new(&project.root).join("keep.txt"), "preserve").unwrap();
    assert!(state
        .create_project("Clear project".to_owned(), None)
        .await
        .unwrap_err()
        .contains("already exists"));
    assert_eq!(
        std::fs::read_to_string(Path::new(&project.root).join("keep.txt")).unwrap(),
        "preserve"
    );
    let thread = state
        .create_thread(
            project.root.clone(),
            ProviderKind::Go,
            Some("minimax-m2.5".to_owned()),
        )
        .await
        .unwrap();
    assert!(thread.worktree_path.is_none());
    assert_eq!(state.get_settings().await.recent_roots[0], project.root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_at_approval_prevents_the_write_and_releases_the_thread() {
    let (_repo, root) = init_repo();
    let (state, _settings) = test_state();
    let server = mount_scripted_llm().await;
    state
        .set_secret("go".to_owned(), "test-key".to_owned())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(server.uri()));
    let thread = state
        .create_thread(
            root.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".to_owned()),
        )
        .await
        .unwrap();
    let (sink, mut events) = ChannelSink::channel();
    state
        .send_message(
            Arc::new(sink),
            thread.id.clone(),
            "write hello.txt".to_owned(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = events.recv().await {
            match event {
                TestEvent::Approval(_) => state.stop_thread(thread.id.clone()).await.unwrap(),
                TestEvent::Thread(envelope) => match envelope.event {
                    ThreadEvent::Failed { error } => {
                        assert!(error.contains("Stopped by you"));
                        return;
                    }
                    ThreadEvent::Finished { .. } => {
                        panic!("stopped run must not finish successfully")
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        panic!("missing terminal event");
    })
    .await
    .unwrap();
    assert!(!state.get_thread(&thread.id).await.unwrap().running);
    assert!(!root.join("hello.txt").exists());
}

#[tokio::test]
async fn shared_workspace_threads_need_no_merge() {
    let (_repo, root) = init_repo();
    let (state, _settings) = test_state();
    let thread = state
        .create_thread(
            root.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test".into()),
        )
        .await
        .unwrap();
    assert!(state
        .merge_thread(thread.id)
        .await
        .unwrap_err()
        .contains("no git worktree"));
}
