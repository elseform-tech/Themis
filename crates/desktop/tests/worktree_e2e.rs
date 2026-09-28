//! Worktree-per-thread tests: lifecycle, isolation, apply-based merge,
//! the concurrency gate, and crash recovery. Real [`AppState`] handlers with
//! temp git repos and a scripted mock LLM (wiremock); no network beyond
//! localhost. Git-dependent tests skip gracefully without the binary.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

mod common;
use common::{git, git_available, init_repo, mount_scripted_llm, use_mock_llm};

use serde_json::{json, Value};
use themis_desktop::sink::{ChannelSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{ApprovalDecision, ProviderKind, ThreadEvent};
use tokio::sync::mpsc::UnboundedReceiver;

/// Overall deadline per wait (fail fast instead of hanging on dialogs).
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

fn git_stdout(repo: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// App state with an explicit worktrees root under `app`. Caller keeps `_app` alive.
fn test_state() -> (AppState, tempfile::TempDir) {
    let app = tempfile::tempdir().expect("tempdir");
    let state = AppState::new_for_test_with_worktrees_root(
        app.path().join("settings.json"),
        app.path().join("worktrees"),
    );
    (state, app)
}

/// Waits for the run's approval dialog, dropping earlier thread events.
async fn wait_for_approval(rx: &mut UnboundedReceiver<TestEvent>) -> String {
    tokio::time::timeout(RUN_TIMEOUT, async {
        while let Some(event) = rx.recv().await {
            if let TestEvent::Approval(request) = event {
                return request.approval_id;
            }
        }
        panic!("event channel closed while waiting for approval");
    })
    .await
    .expect("approval arrived before the deadline")
}

/// Drains events to the terminal one, answering stray approvals (there are none).
async fn finish_run(
    state: &AppState,
    rx: &mut UnboundedReceiver<TestEvent>,
    thread_id: &str,
    run_id: &str,
) -> Vec<ThreadEvent> {
    let mut events = Vec::new();
    tokio::time::timeout(RUN_TIMEOUT, async {
        while let Some(event) = rx.recv().await {
            match event {
                TestEvent::Approval(request) => {
                    state
                        .approve_action(
                            thread_id.to_owned(),
                            request.approval_id.clone(),
                            ApprovalDecision::Once,
                        )
                        .await
                        .expect("approve");
                }
                TestEvent::Thread(envelope) => {
                    assert_eq!(envelope.thread_id, thread_id);
                    assert_eq!(envelope.run_id, run_id);
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
    .await
    .expect("run finished before the deadline");
    events
}

#[tokio::test]
async fn threads_share_checkout_and_removing_thread_preserves_files() {
    let (_repo, root) = init_repo();
    let (state, app) = test_state();
    std::fs::write(root.join("existing.txt"), "user work").unwrap();
    let mut threads = Vec::new();
    for _ in 0..2 {
        let thread = state
            .create_thread(root.to_string_lossy().into_owned(), ProviderKind::Go, None)
            .await
            .unwrap();
        assert!(thread.worktree_path.is_none());
        assert!(thread.branch.is_none());
        threads.push(thread);
    }
    assert!(!app.path().join("worktrees").exists());
    assert!(git_stdout(&root, &["branch", "--list", "themis/*"]).is_empty());
    std::fs::write(root.join("shared.txt"), "shared edit").unwrap();
    for thread in &threads {
        let diff = state.list_diff(thread.id.clone()).await.unwrap();
        assert!(diff.files.iter().any(|file| file.path == "shared.txt"));
        assert!(diff
            .files
            .iter()
            .any(|file| file.path == "existing.txt" && file.preexisting));
        assert!(state
            .discard_file(thread.id.clone(), "existing.txt".into())
            .await
            .is_err());
    }
    state.discard_thread(threads[0].id.clone()).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("shared.txt")).unwrap(),
        "shared edit"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("existing.txt")).unwrap(),
        "user work"
    );
}

#[tokio::test]
async fn merge_and_discard_reject_busy_threads() {
    if !git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    let (_repo_temp, root) = init_repo();
    let (state, _app) = test_state();
    let server = mount_scripted_llm().await;
    use_mock_llm(&state, &server).await;
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
    let approval_id = wait_for_approval(&mut rx).await;

    let err = state
        .merge_thread(thread.id.clone())
        .await
        .expect_err("busy merge rejected");
    assert!(err.contains("busy"), "{err}");
    let err = state
        .discard_thread(thread.id.clone())
        .await
        .expect_err("busy discard rejected");
    assert!(err.contains("busy"), "{err}");

    state
        .approve_action(thread.id.clone(), approval_id, ApprovalDecision::Once)
        .await
        .expect("approve");
    let events = finish_run(&state, &mut rx, &thread.id, &handle.run_id).await;
    assert!(matches!(events.last(), Some(ThreadEvent::Finished { .. })));

    // Idle again, discard works.
    state
        .discard_thread(thread.id.clone())
        .await
        .expect("discard after run");
}

#[tokio::test]
async fn concurrency_gate_rejects_when_saturated() {
    if !git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    let (_repo_temp, root) = init_repo();
    let (state, _app) = test_state();
    assert_eq!(state.get_settings().await.concurrency_limit, 3);
    let server = mount_scripted_llm().await;
    use_mock_llm(&state, &server).await;
    let mut threads = Vec::new();
    for _ in 0..4 {
        threads.push(
            state
                .create_thread(
                    root.to_string_lossy().into_owned(),
                    ProviderKind::Go,
                    Some("test-model".to_owned()),
                )
                .await
                .expect("create thread"),
        );
    }

    // Occupy all three permits with runs blocked on their approval dialogs.
    let mut blocked = Vec::new();
    for thread in &threads[..3] {
        let (sink, mut rx) = ChannelSink::channel();
        let handle = state
            .send_message(
                Arc::new(sink),
                thread.id.clone(),
                "write hello.txt".to_owned(),
            )
            .await
            .expect("send spawns");
        let approval_id = wait_for_approval(&mut rx).await;
        blocked.push((thread.id.clone(), handle.run_id, rx, approval_id));
    }

    // The fourth send is rejected, not queued.
    let (sink, _rx) = ChannelSink::channel();
    let err = state
        .send_message(Arc::new(sink), threads[3].id.clone(), "late".to_owned())
        .await
        .expect_err("saturated gate rejects");
    assert!(err.contains("concurrency limit reached (3)"), "{err}");

    // Releasing the runs frees the permits: each finishes normally...
    for (thread_id, run_id, mut rx, approval_id) in blocked {
        state
            .approve_action(thread_id.clone(), approval_id, ApprovalDecision::Once)
            .await
            .expect("approve");
        let events = finish_run(&state, &mut rx, &thread_id, &run_id).await;
        assert!(matches!(events.last(), Some(ThreadEvent::Finished { .. })));
    }
    // ... and a new send spawns again.
    let (sink, mut rx) = ChannelSink::channel();
    let handle = state
        .send_message(Arc::new(sink), threads[3].id.clone(), "now".to_owned())
        .await
        .expect("permit released");
    let approval_id = wait_for_approval(&mut rx).await;
    state
        .approve_action(threads[3].id.clone(), approval_id, ApprovalDecision::Once)
        .await
        .expect("approve");
    let events = finish_run(&state, &mut rx, &threads[3].id, &handle.run_id).await;
    assert!(matches!(events.last(), Some(ThreadEvent::Finished { .. })));
}

#[tokio::test]
async fn concurrency_limit_setting_is_honored() {
    if !git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    let (_repo_temp, root) = init_repo();
    let (state, _app) = test_state();
    let server = mount_scripted_llm().await;
    use_mock_llm(&state, &server).await;
    let updated = state
        .update_settings(themis_desktop::types::SettingsPatch {
            concurrency_limit: Some(1),
            ..Default::default()
        })
        .await
        .expect("update limit");
    assert_eq!(updated.concurrency_limit, 1);

    let first = state
        .create_thread(
            root.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".to_owned()),
        )
        .await
        .expect("create thread");
    let second = state
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
            first.id.clone(),
            "write hello.txt".to_owned(),
        )
        .await
        .expect("first send spawns");
    let approval_id = wait_for_approval(&mut rx).await;

    let (sink, _rx) = ChannelSink::channel();
    let err = state
        .send_message(Arc::new(sink), second.id.clone(), "second".to_owned())
        .await
        .expect_err("limit 1 rejects the second run");
    assert!(err.contains("concurrency limit reached (1)"), "{err}");

    state
        .approve_action(first.id.clone(), approval_id, ApprovalDecision::Once)
        .await
        .expect("approve");
    let events = finish_run(&state, &mut rx, &first.id, &handle.run_id).await;
    assert!(matches!(events.last(), Some(ThreadEvent::Finished { .. })));
}

#[tokio::test]
async fn recovery_marks_running_threads_recovered() {
    if !git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    let (_repo_temp, root) = init_repo();
    let app = tempfile::tempdir().expect("tempdir");
    let settings_path = app.path().join("settings.json");
    let worktrees_root = app.path().join("worktrees");
    let state =
        AppState::new_for_test_with_worktrees_root(settings_path.clone(), worktrees_root.clone());
    let thread = state
        .create_thread(
            root.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("model".to_owned()),
        )
        .await
        .expect("create thread");
    let worktree = root.to_string_lossy().into_owned();
    std::fs::write(Path::new(&worktree).join("draft.txt"), "unfinished\n").expect("write");

    // Simulate a crash mid-run: flip `was_running`, then reboot in place.
    let registry_path = app.path().join("threads.json");
    let mut registry: Value =
        serde_json::from_str(&std::fs::read_to_string(&registry_path).expect("read"))
            .expect("parse");
    registry[0]["was_running"] = json!(true);
    registry[0]["title"] = json!("my draft");
    std::fs::write(
        &registry_path,
        serde_json::to_string_pretty(&registry).expect("encode"),
    )
    .expect("write");
    drop(state);
    let rebooted =
        AppState::new_for_test_with_worktrees_root(settings_path, worktrees_root.clone());

    let info = rebooted.get_thread(&thread.id).await.expect("restored");
    assert!(!info.running, "reconciled to idle");
    assert!(info.recovered, "flagged as recovered");
    assert_eq!(info.title, "my draft");
    assert!(info.worktree_path.is_none());
    let report = rebooted.reconcile_report().join("\n");
    assert!(report.contains("recovered"), "{report}");

    // The worktree is still usable: old work visible, new work listed.
    let diff = rebooted.list_diff(thread.id.clone()).await.expect("diff");
    assert!(diff.available);
    assert!(diff.files.iter().any(|file| file.path == "draft.txt"));
    std::fs::write(Path::new(&worktree).join("more.txt"), "x\n").expect("write");
    let diff = rebooted.list_diff(thread.id.clone()).await.expect("diff");
    assert!(diff.files.iter().any(|file| file.path == "more.txt"));

    // The reconciled registry cleared `was_running`.
    let registry: Value =
        serde_json::from_str(&std::fs::read_to_string(&registry_path).expect("read"))
            .expect("parse");
    assert_eq!(registry[0]["was_running"], json!(false));
}

#[tokio::test]
async fn legacy_threads_resume_in_shared_checkout_and_keep_old_work() {
    let (_repo, root) = init_repo();
    let (state, app) = test_state();
    let thread = state
        .create_thread(
            root.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".into()),
        )
        .await
        .unwrap();
    let legacy = app.path().join("worktrees").join(&thread.id);
    git(
        &root,
        &[
            "worktree",
            "add",
            "--detach",
            legacy.to_str().unwrap(),
            "HEAD",
        ],
    );
    std::fs::write(legacy.join("keep.txt"), "legacy work").unwrap();
    let registry_path = app.path().join("threads.json");
    let mut registry: Value =
        serde_json::from_str(&std::fs::read_to_string(&registry_path).unwrap()).unwrap();
    registry[0]["worktree_path"] = json!(legacy);
    std::fs::write(&registry_path, serde_json::to_string(&registry).unwrap()).unwrap();
    drop(state);
    let state = AppState::new_for_test_with_worktrees_root(
        app.path().join("settings.json"),
        app.path().join("worktrees"),
    );
    assert!(state
        .get_thread(&thread.id)
        .await
        .unwrap()
        .worktree_path
        .is_none());
    let server = mount_scripted_llm().await;
    use_mock_llm(&state, &server).await;
    let (sink, mut rx) = ChannelSink::channel();
    let run = state
        .send_message(Arc::new(sink), thread.id.clone(), "write hello.txt".into())
        .await
        .unwrap();
    let events = finish_run(&state, &mut rx, &thread.id, &run.run_id).await;
    assert!(matches!(events.last(), Some(ThreadEvent::Finished { .. })));
    assert!(root.join("hello.txt").exists());
    assert!(!legacy.join("hello.txt").exists());
    state.discard_thread(thread.id).await.unwrap();
    drop(state);
    let _reboot = AppState::new_for_test_with_worktrees_root(
        app.path().join("settings.json"),
        app.path().join("worktrees"),
    );
    assert_eq!(
        std::fs::read_to_string(legacy.join("keep.txt")).unwrap(),
        "legacy work"
    );
}

#[tokio::test]
async fn boot_preserves_legacy_worktrees() {
    if !git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    let (_repo_temp, root) = init_repo();
    let app = tempfile::tempdir().expect("tempdir");
    let worktrees_root = app.path().join("worktrees");
    std::fs::create_dir_all(&worktrees_root).expect("mkdir");

    // A genuine orphan: a linked worktree no registry references.
    let orphan = "12345678-9abc-4def-8123-456789abcdef".to_owned();
    git(
        &root,
        &[
            "worktree",
            "add",
            &worktrees_root.join(&orphan).to_string_lossy(),
            "HEAD",
        ],
    );
    // Foreign bodies: never ours, never touched.
    let foreign = worktrees_root.join("someone-elses-dir");
    std::fs::create_dir(&foreign).expect("mkdir");
    std::fs::write(foreign.join("keep.txt"), "keep\n").expect("write");
    let fake = worktrees_root.join("87654321-9abc-4def-8123-456789abcdef");
    std::fs::create_dir(&fake).expect("mkdir");

    let state = AppState::new_for_test_with_worktrees_root(
        app.path().join("settings.json"),
        worktrees_root.clone(),
    );
    assert!(
        worktrees_root.join(&orphan).exists(),
        "legacy worktree retained"
    );
    assert!(foreign.join("keep.txt").is_file(), "foreign dir kept");
    assert!(fake.is_dir(), "non-worktree uuid dir kept");
    assert!(state.reconcile_report().is_empty());
}
