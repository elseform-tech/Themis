use super::*;
use crate::sink::TestEvent;
use crate::types::{AutomationOrigin, SettingsPatch, ThemeMode};

fn test_state() -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    (state, dir)
}

#[tokio::test]
async fn a_lagging_browser_view_catches_up_from_the_current_state() {
    let (state, dir) = test_state();
    assert!(state.browser_state_update("t1").await.is_none());
    state.browser_of("t1", dir.path());
    match state.browser_state_update("t1").await {
        Some(themis_core::browser::BrowserUpdate::State(browser)) => {
            assert_eq!(browser.activity, None);
            assert_eq!(browser.controller, "idle");
        }
        other => panic!("expected the current state, got {other:?}"),
    }
}

#[tokio::test]
async fn open_project_rejects_missing_or_file_paths() {
    let (state, dir) = test_state();
    assert!(state.open_project(String::new()).await.is_err());
    assert!(state
        .open_project(dir.path().join("nope").to_string_lossy().into_owned())
        .await
        .is_err());
    let file = dir.path().join("file.txt");
    std::fs::write(&file, "x").expect("write");
    assert!(state
        .open_project(file.to_string_lossy().into_owned())
        .await
        .is_err());
    let info = state
        .open_project(dir.path().to_string_lossy().into_owned())
        .await
        .expect("open");
    assert!(!info.root.is_empty());
    assert!(!info.is_git);
}

#[tokio::test]
async fn unknown_ids_error_clearly() {
    let (state, _dir) = test_state();
    for result in [
        state.list_diff("nope".to_owned()).await.map(|_| ()),
        state.accept_file("nope".to_owned(), "a".to_owned()).await,
        state.discard_file("nope".to_owned(), "a".to_owned()).await,
        state
            .add_comment("nope".to_owned(), "a".to_owned(), "c".to_owned())
            .await
            .map(|_| ()),
        state
            .set_provider("nope".to_owned(), ProviderKind::Go, None)
            .await
            .map(|_| ()),
        state.get_thread("nope").await.map(|_| ()),
        state.merge_thread("nope".to_owned()).await.map(|_| ()),
        state.discard_thread("nope".to_owned()).await,
    ] {
        let err = result.expect_err("unknown thread");
        assert!(err.contains("unknown thread"), "{err}");
    }
}

#[tokio::test]
async fn non_git_threads_have_no_worktree_and_drop_cleanly() {
    let (state, dir) = test_state();
    let project = dir.path().join("plain");
    std::fs::create_dir(&project).expect("mkdir");
    let thread = state
        .create_thread(
            project.to_string_lossy().into_owned(),
            ProviderKind::Go,
            None,
        )
        .await
        .expect("create");
    assert!(thread.worktree_path.is_none());
    assert!(thread.branch.is_none());
    assert!(thread.base_branch.is_none());
    assert!(!thread.recovered);
    let err = state
        .merge_thread(thread.id.clone())
        .await
        .expect_err("non-git merge rejected");
    assert!(err.contains("no git worktree"), "{err}");
    state
        .discard_thread(thread.id.clone())
        .await
        .expect("non-git discard drops");
    assert!(state.get_thread(&thread.id).await.is_err());
}

#[tokio::test]
async fn list_threads_filters_by_project() {
    let (state, dir) = test_state();
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    std::fs::create_dir(&first).expect("mkdir");
    std::fs::create_dir(&second).expect("mkdir");
    let thread_a = state
        .create_thread(first.to_string_lossy().into_owned(), ProviderKind::Go, None)
        .await
        .expect("create");
    let thread_b = state
        .create_thread(
            second.to_string_lossy().into_owned(),
            ProviderKind::Go,
            None,
        )
        .await
        .expect("create");
    let listed = state
        .list_threads(first.to_string_lossy().into_owned())
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, thread_a.id);
    let listed = state
        .list_threads(second.to_string_lossy().into_owned())
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, thread_b.id);
    let empty = dir.path().join("empty");
    std::fs::create_dir(&empty).expect("mkdir");
    let listed = state
        .list_threads(empty.to_string_lossy().into_owned())
        .await
        .expect("list");
    assert!(listed.is_empty());
    assert!(state.list_threads(String::new()).await.is_err());
}

#[tokio::test]
async fn open_project_tracks_recent_roots() {
    let (state, dir) = test_state();
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    std::fs::create_dir(&first).expect("mkdir");
    std::fs::create_dir(&second).expect("mkdir");
    state
        .open_project(first.to_string_lossy().into_owned())
        .await
        .expect("open");
    state
        .open_project(second.to_string_lossy().into_owned())
        .await
        .expect("open");
    state
        .open_project(first.to_string_lossy().into_owned())
        .await
        .expect("reopen");
    let settings = state.get_settings().await;
    let first_root = first.canonicalize().expect("canonical");
    let second_root = second.canonicalize().expect("canonical");
    assert_eq!(
        settings.recent_roots,
        vec![
            first_root.to_string_lossy().into_owned(),
            second_root.to_string_lossy().into_owned(),
        ]
    );
}

#[tokio::test]
async fn create_thread_persists_the_registry() {
    let (state, dir) = test_state();
    let thread = state
        .create_thread(
            dir.path().to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("gpt-x".to_owned()),
        )
        .await
        .expect("create");
    let registry = dir.path().join("threads.json");
    assert!(registry.is_file(), "registry was written");
    let text = std::fs::read_to_string(&registry).expect("read");
    assert!(text.contains(&thread.id), "{text}");
    assert!(text.contains("\"was_running\": false"), "{text}");
    // A reboot from the same dir restores the thread, idle and unrecovered.
    drop(state);
    let rebooted = AppState::new_for_test(dir.path().join("settings.json"));
    let info = rebooted.get_thread(&thread.id).await.expect("restored");
    assert_eq!(info.title, UNTITLED_THREAD);
    assert!(!info.running);
    assert!(!info.recovered);
    assert!(rebooted.reconcile_report().is_empty());
}

#[tokio::test]
async fn cold_effort_validation_loads_catalog_before_rejecting_support() {
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };
    for setting in [true, false] {
        let (state, dir) = test_state();
        state
            .set_secret("go".into(), "synthetic-test-key".into())
            .await
            .unwrap();
        let server = MockServer::builder().start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&server)
            .await;
        state.set_go_base_url_override(Some(server.uri()));
        let thread = state
            .create_thread(
                dir.path().to_string_lossy().into_owned(),
                ProviderKind::Go,
                Some("model-a".into()),
            )
            .await
            .unwrap();
        let error = if setting {
            state
                .set_thread_effort(thread.id.clone(), Some("low".into()))
                .await
                .unwrap_err()
        } else {
            let (sink, _) = crate::sink::ChannelSink::channel();
            state
                .send_message_with_effort(
                    Arc::new(sink),
                    thread.id.clone(),
                    "hello".into(),
                    Some("low".into()),
                )
                .await
                .unwrap_err()
        };
        assert!(
            error.contains("Could not load OpenCode Go models"),
            "an unloaded catalog is not evidence of unsupported effort: {error}"
        );
        assert!(!state.get_thread(&thread.id).await.unwrap().running);
    }
}

#[tokio::test]
async fn stop_cancels_cold_catalog_load_without_holding_thread_locks() {
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };
    let (state, dir) = test_state();
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    let server = MockServer::builder().start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(503).set_delay(std::time::Duration::from_secs(30)))
        .mount(&server)
        .await;
    state.set_go_base_url_override(Some(server.uri()));
    let thread = state
        .create_thread(
            dir.path().to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("model-a".into()),
        )
        .await
        .unwrap();
    let copy = state.clone();
    let id = thread.id.clone();
    let mut sending = tokio::spawn(async move {
        let (sink, _) = crate::sink::ChannelSink::channel();
        copy.send_message_with_effort(Arc::new(sink), id, "hello".into(), Some("low".into()))
            .await
    });
    // Admission includes filesystem skill discovery; parallel suite load must
    // not consume the separate, strict catalog-cancellation allowance below.
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
            while server.received_requests().await.unwrap().is_empty() {
                tokio::select! {
                    biased;
                    result = &mut sending => panic!("send ended before the catalog request: {result:?}"),
                    _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {},
                }
            }
        })
        .await
        .expect("catalog fetch started after run preparation");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        assert!(state.get_thread(&thread.id).await.unwrap().running);
        state.stop_thread(thread.id.clone()).await.unwrap();
        let error = sending.await.unwrap().unwrap_err();
        assert!(error.contains("Stopped by you"), "{error}");
        assert!(!state.get_thread(&thread.id).await.unwrap().running);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_model_the_catalog_marks_unsupported_is_refused_before_any_request() {
    use wiremock::MockServer;
    let (state, dir) = test_state();
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    let server = MockServer::builder().start().await;
    state.set_go_base_url_override(Some(server.uri()));
    let thread = state
        .create_thread(
            dir.path().to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("strange".to_owned()),
        )
        .await
        .expect("create");
    *state.inner.go_catalog.write().await = vec![themis_core::providers::GoModel {
        id: "strange".to_owned(),
        protocol: Some(themis_core::providers::Protocol::Unsupported),
        unavailable_reason: Some("Needs @ai-sdk/xyz, which Themis does not support yet".into()),
        ..Default::default()
    }];
    let (sink, _) = crate::sink::ChannelSink::channel();
    let error = state
        .send_message(Arc::new(sink), thread.id.clone(), "hello".into())
        .await
        .unwrap_err();
    assert!(
        error.contains("is unavailable: Needs @ai-sdk/xyz"),
        "{error}"
    );
    assert!(!state.get_thread(&thread.id).await.unwrap().running);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_retired_default_model_is_replaced_but_a_chosen_model_stands() {
    let (state, dir) = test_state();
    *state.inner.go_catalog.write().await = vec![themis_core::providers::GoModel {
        id: "glm-5.3".to_owned(),
        ..Default::default()
    }];
    let root = dir.path().to_string_lossy().into_owned();
    let default = state
        .create_thread(root.clone(), ProviderKind::Go, None)
        .await
        .unwrap();
    assert_eq!(
        default.model, "glm-5.3",
        "the built-in default is gone from Go"
    );
    let chosen = state
        .create_thread(root, ProviderKind::Go, Some("retired-model".to_owned()))
        .await
        .unwrap();
    assert_eq!(chosen.model, "retired-model");
}

#[tokio::test]
async fn thread_effort_follows_catalog_and_survives_restart() {
    let (state, dir) = test_state();
    let thread = state
        .create_thread(
            dir.path().to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("model-a".to_owned()),
        )
        .await
        .expect("create");
    *state.inner.go_catalog.write().await = vec![themis_core::providers::GoModel {
        id: "model-a".to_owned(),
        effort_levels: vec!["low".to_owned(), "high".to_owned()],
        context_tokens: None,
        output_tokens: None,
        thinking_toggle: false,
        ..Default::default()
    }];
    assert!(state
        .set_thread_effort(thread.id.clone(), Some("max".to_owned()))
        .await
        .is_err());
    let updated = state
        .set_thread_effort(thread.id.clone(), Some("high".to_owned()))
        .await
        .expect("catalog effort");
    assert_eq!(updated.reasoning_effort.as_deref(), Some("high"));
    drop(state);
    let rebooted = AppState::new_for_test(dir.path().join("settings.json"));
    let restored = rebooted.get_thread(&thread.id).await.expect("restored");
    assert_eq!(restored.reasoning_effort.as_deref(), Some("high"));
    let cleared = rebooted
        .set_thread_effort(thread.id.clone(), Some(String::new()))
        .await
        .expect("default effort");
    assert_eq!(cleared.reasoning_effort, None);
}

#[tokio::test]
async fn send_message_requires_a_stored_key_and_resets_running() {
    let (state, dir) = test_state();
    let thread = state
        .create_thread(
            dir.path().to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("gpt-x".to_owned()),
        )
        .await
        .expect("create");
    let (sink, mut events) = crate::sink::ChannelSink::channel();
    let err = state
        .send_message(Arc::new(sink), thread.id.clone(), "hi".to_owned())
        .await
        .expect_err("missing key");
    assert!(err.contains("no API key"), "{err}");
    let crate::sink::TestEvent::Thread(message) = events.try_recv().expect("the saved message")
    else {
        panic!("expected the user message first");
    };
    assert!(matches!(message.event, ThreadEvent::UserMessage { .. }));
    let crate::sink::TestEvent::Thread(started) = events.try_recv().expect("run admission") else {
        panic!("expected run admission event");
    };
    assert!(matches!(started.event, ThreadEvent::Started { .. }));
    let crate::sink::TestEvent::Thread(failed) = events.try_recv().expect("setup failure") else {
        panic!("expected setup failure event");
    };
    assert_eq!(started.run_id, failed.run_id);
    assert!(matches!(failed.event, ThreadEvent::Failed { .. }));
    assert!(events.try_recv().is_err(), "one start and one terminal");
    // The failure path resets `running`: a retry fails the same way, not busy.
    let (sink, _rx) = crate::sink::ChannelSink::channel();
    let retry = state
        .send_message(Arc::new(sink), thread.id.clone(), "hi".to_owned())
        .await
        .expect_err("missing key again");
    assert!(retry.contains("no API key"), "{retry}");
    assert!(!state.get_thread(&thread.id).await.expect("info").running);
}

#[tokio::test]
async fn go_uses_default_model() {
    let (state, dir) = test_state();
    let thread = state
        .create_thread(
            dir.path().to_string_lossy().into_owned(),
            ProviderKind::Go,
            None,
        )
        .await
        .expect("create");
    assert_eq!(thread.model, GO_DEFAULT_MODEL);
}

#[test]
fn a_goal_chat_is_named_after_the_goal() {
    assert_eq!(
        title_from_message(
            "[Goal — runtime, not from the user] Turn 1. Goal: speed up p95\nLast check: x"
        ),
        "speed up p95"
    );
    assert_eq!(title_from_message("/goal speed up p95"), "speed up p95");
    assert_eq!(title_from_message("/goals list"), "/goals list");
}

#[test]
fn skill_and_plugin_tokens_stay_out_of_titles() {
    assert_eq!(
        title_from_message("[[skill:browser-use]] Open http://127.0.0.1/ and check it"),
        "Open http://127.0.0.1/ and check it"
    );
    assert_eq!(
        title_from_message("Review [[plugin:github]] the  PR"),
        "Review the PR"
    );
    assert_eq!(title_from_message("[[skill:browser-use]]"), "browser-use");
}

proptest::proptest! {
    #[test]
    fn any_first_message_makes_a_bounded_title_without_tokens(
        text in "(\\[\\[|\\]\\]|skill:|plugin:|[a-z-]{0,6}| |\n|é){0,40}",
    ) {
        let title = title_from_message(&text);
        proptest::prop_assert!(!title.is_empty());
        proptest::prop_assert!(title.chars().count() <= MAX_TITLE_CHARS);
        proptest::prop_assert!(!title.contains('\n'));
        let (words, _) = without_tokens(&title);
        proptest::prop_assert_eq!(&words, &title, "a token survived");
    }
}

#[tokio::test]
async fn first_message_sets_a_truncated_title() {
    let (state, dir) = test_state();
    state
        .set_secret("go".to_owned(), "synthetic-test-key".to_owned())
        .await
        .unwrap();
    let thread = state
        .create_thread(
            dir.path().to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".to_owned()),
        )
        .await
        .expect("create");
    assert_eq!(thread.title, UNTITLED_THREAD);
    // The run may fail without a mock endpoint; title assignment is synchronous.
    let (sink, _rx) = crate::sink::ChannelSink::channel();
    let long = format!("{} and more words after the limit", "w".repeat(80));
    let handle = state
        .send_message(Arc::new(sink), thread.id.clone(), long)
        .await
        .expect("spawn");
    assert!(!handle.run_id.is_empty());
    // The title is set synchronously, before the run resolves.
    let info = state.get_thread(&thread.id).await.expect("info");
    assert_eq!(info.title.chars().count(), MAX_TITLE_CHARS);
    assert!(info.title.chars().all(|char| char == 'w' || char == ' '));
    // Busy-rejection is covered deterministically in tests/bridge_e2e.rs,
    // where the first run blocks on an approval dialog.
}

#[tokio::test]
async fn approve_action_rejects_unknown_ids() {
    let (state, dir) = test_state();
    let thread = state
        .create_thread(
            dir.path().to_string_lossy().into_owned(),
            ProviderKind::Go,
            None,
        )
        .await
        .expect("create");
    let err = state
        .approve_action(
            thread.id.clone(),
            "missing".to_owned(),
            ApprovalDecision::Once,
        )
        .await
        .expect_err("unknown approval");
    assert!(err.contains("unknown or expired"), "{err}");
    let err = state
        .approve_action(
            "missing-thread".to_owned(),
            "missing".to_owned(),
            ApprovalDecision::Deny,
        )
        .await
        .expect_err("unknown thread");
    assert!(err.contains("unknown thread"), "{err}");
}

#[tokio::test]
async fn comments_and_settings_flow() {
    let (state, dir) = test_state();
    let thread = state
        .create_thread(
            dir.path().to_string_lossy().into_owned(),
            ProviderKind::Go,
            None,
        )
        .await
        .expect("create");
    let comment = state
        .add_comment(thread.id.clone(), "a.rs".to_owned(), "look".to_owned())
        .await
        .expect("comment");
    assert_eq!(comment.path, "a.rs");
    assert_eq!(comment.comment, "look");
    assert!(!comment.id.is_empty());
    // `accept_file` records without touching the disk.
    state
        .accept_file(thread.id.clone(), "a.rs".to_owned())
        .await
        .expect("accept");

    let settings = state.get_settings().await;
    assert_eq!(
        settings,
        Settings {
            projects_directory: settings_app_dir(state.inner.settings.path())
                .join("projects")
                .to_string_lossy()
                .into_owned(),
            // Test state starts chats in Custom; the app's own default is Auto.
            default_approval_mode: themis_core::configuration::ApprovalMode::Custom,
            ..Settings::default()
        }
    );
    let updated = state
        .update_settings(SettingsPatch {
            theme: Some(ThemeMode::Light),
            context_token_cap: Some(300_000),
            ..SettingsPatch::default()
        })
        .await
        .expect("update");
    assert_eq!(updated.theme, ThemeMode::Dark);
    assert_eq!(updated.context_token_cap, 300_000);
    let bad = state
        .update_settings(SettingsPatch {
            context_token_cap: Some(10),
            ..SettingsPatch::default()
        })
        .await
        .expect_err("validated");
    assert!(bad.contains("context_token_cap"), "{bad}");
}

#[tokio::test]
async fn secrets_never_surface_values() {
    let (state, _dir) = test_state();
    let status = state.get_secret_status().await.unwrap();
    assert_eq!(status, SecretStatus { go: false });
    assert!(state
        .set_secret("go".to_owned(), String::new())
        .await
        .is_err());
    assert!(state
        .set_secret("ollama".to_owned(), "x".to_owned())
        .await
        .is_err());
    state
        .set_secret("go".to_owned(), "sk-test".to_owned())
        .await
        .expect("set");
    let status = state.get_secret_status().await.unwrap();
    assert!(status.go);
    // The status JSON carries presence only.
    let json = serde_json::to_string(&status).expect("json");
    assert!(!json.contains("sk-test"));
    state.clear_secret("go".to_owned()).await.expect("clear");
    assert!(!state.get_secret_status().await.unwrap().go);
}

#[tokio::test]
async fn non_git_diff_is_unavailable() {
    let (state, dir) = test_state();
    let thread = state
        .create_thread(
            dir.path().to_string_lossy().into_owned(),
            ProviderKind::Go,
            None,
        )
        .await
        .expect("create");
    let diff = state.list_diff(thread.id).await.expect("diff");
    assert!(!diff.available);
    assert!(diff.reason.is_some());
    assert!(diff.files.is_empty());
}

#[tokio::test]
async fn discard_rejects_escapes() {
    let (state, dir) = test_state();
    let thread = state
        .create_thread(
            dir.path().to_string_lossy().into_owned(),
            ProviderKind::Go,
            None,
        )
        .await
        .expect("create");
    for bad in ["../evil.txt", "/abs/path.txt", "sub/../../evil.txt"] {
        let err = state
            .discard_file(thread.id.clone(), bad.to_owned())
            .await
            .expect_err("escape rejected");
        assert!(err.contains("relative"), "{err}");
    }
}

fn skill_input(name: &str) -> SkillInput {
    SkillInput {
        name: name.to_owned(),
        description: format!("{name} description"),
        instructions: "Be helpful.".to_owned(),
        allowed_tools: Vec::new(),
        scripts: Vec::new(),
    }
}

fn automation_input(project_root: &str) -> AutomationInput {
    AutomationInput {
        name: "Nightly".to_owned(),
        project_root: project_root.to_owned(),
        provider: ProviderKind::Go,
        model: "test-model".to_owned(),
        reasoning_effort: None,
        target_thread_id: None,
        skill_ids: Vec::new(),
        interval_mins: 60,
        schedule: None,
        task: "Check health.".to_owned(),
        enabled: true,
        watch: Default::default(),
    }
}

fn test_sink() -> (
    Arc<dyn EventSink>,
    tokio::sync::mpsc::UnboundedReceiver<TestEvent>,
) {
    let (sink, rx) = crate::sink::ChannelSink::channel();
    (Arc::new(sink), rx)
}

#[tokio::test]
async fn thread_tasks_report_live_automations_and_finished_runs() {
    let (state, dir) = test_state();
    let project = dir.path().to_string_lossy().into_owned();
    let thread = state
        .create_thread(project.clone(), ProviderKind::Go, Some("m".to_owned()))
        .await
        .expect("thread");
    let other = state
        .create_thread(project.clone(), ProviderKind::Go, Some("m".to_owned()))
        .await
        .expect("other thread");
    let mut input = automation_input(&project);
    input.target_thread_id = Some(thread.id.clone());
    let automation = state
        .create_automation_from(input.clone(), AutomationOrigin::Agent)
        .await
        .expect("automation");
    state.create_automation(input).await.expect("user one");
    let tasks = state.get_thread_tasks(&thread.id).await.expect("tasks");
    assert_eq!(tasks.automations.len(), 2);
    assert!(tasks
        .automations
        .iter()
        .any(|item| item.origin == AutomationOrigin::Agent));
    assert!(!tasks.run.running && !tasks.run.awaiting_approval);
    assert!(state
        .get_thread_tasks(&other.id)
        .await
        .expect("other tasks")
        .automations
        .is_empty());

    state.inner.automation_runs.lock().unwrap().insert(
        "run-1".to_owned(),
        AutomationRunContext {
            automation_id: automation.id.clone(),
            thread_id: thread.id.clone(),
            title: "Automation Nightly".to_owned(),
            source: Default::default(),
            check_hash: None,
        },
    );
    let live = state.get_thread_tasks(&thread.id).await.expect("tasks");
    assert_eq!(
        live.automations.iter().filter(|item| item.running).count(),
        1
    );
    let (sink, _rx) = test_sink();
    state
        .finish_automation_run(&sink, "run-1", "failed: boom".to_owned())
        .await;
    let done = state.get_thread_tasks(&thread.id).await.expect("tasks");
    assert!(done.automations.iter().all(|item| !item.running));
    assert_eq!(done.recent.len(), 1);
    assert!(!done.recent[0].ok && !done.recent[0].seen);
    assert_eq!(done.recent[0].name, "Nightly");
    assert!(state.get_thread_tasks("missing").await.is_err());
}

#[tokio::test]
async fn thread_tasks_classify_runs_from_terminal_events() {
    let (state, dir) = test_state();
    let project = dir.path().to_string_lossy().into_owned();
    let thread = state
        .create_thread(project.clone(), ProviderKind::Go, Some("m".to_owned()))
        .await
        .expect("thread");
    let mut input = automation_input(&project);
    input.target_thread_id = Some(thread.id.clone());
    let automation = state.create_automation(input).await.expect("automation");
    let (sink, _rx) = test_sink();
    for (run_id, event) in [
        (
            "ok-run",
            RunEvent::Finished {
                result: "done".to_owned(),
            },
        ),
        (
            "bad-run",
            RunEvent::Failed {
                error: "boom".to_owned(),
            },
        ),
    ] {
        state.inner.automation_runs.lock().unwrap().insert(
            run_id.to_owned(),
            AutomationRunContext {
                automation_id: automation.id.clone(),
                thread_id: thread.id.clone(),
                title: "t".to_owned(),
                source: Default::default(),
                check_hash: None,
            },
        );
        state.complete_automation_run(&sink, run_id, &event).await;
    }
    let tasks = state.get_thread_tasks(&thread.id).await.expect("tasks");
    assert_eq!(tasks.recent.len(), 2);
    assert_eq!(tasks.recent.iter().filter(|run| run.ok).count(), 1);
    let times: Vec<_> = tasks.recent.iter().map(|run| &run.finished_at).collect();
    assert!(times[0] >= times[1], "newest first");
}

#[tokio::test]
async fn skill_crud_validates_and_persists() {
    let (state, dir) = test_state();
    assert!(state.list_skills().await.is_empty());

    // Unknown tools fail with an error naming the bad tool AND the valid names.
    let mut bad = skill_input("Bad");
    bad.allowed_tools = vec!["bogus_tool".to_owned()];
    let err = state
        .create_skill(bad)
        .await
        .expect_err("unknown tool rejected");
    assert!(err.contains("bogus_tool"), "{err}");
    for valid in ["read_file", "write_file", "shell", "apply_patch"] {
        assert!(err.contains(valid), "{valid} missing from: {err}");
    }
    // Listing is the shell's, except on Windows, which keeps the tool.
    assert_eq!(err.contains("list_dir"), cfg!(windows), "{err}");
    // Empty names and unsafe script names fail too.
    assert!(state.create_skill(skill_input("   ")).await.is_err());
    let mut bad_script = skill_input("BadScript");
    bad_script.scripts = vec![crate::types::SkillScript {
        name: "../escape.sh".to_owned(),
        content: "pwn".to_owned(),
    }];
    let err = state
        .create_skill(bad_script)
        .await
        .expect_err("unsafe script rejected");
    assert!(err.contains("unsafe script name"), "{err}");

    let created = state
        .create_skill(skill_input("Rust"))
        .await
        .expect("create");
    assert!(!created.id.is_empty());
    assert_eq!(created.name, "Rust");
    assert_eq!(state.list_skills().await.len(), 1);

    let mut updated_input = skill_input("Rustacean");
    updated_input.allowed_tools = vec!["read_file".to_owned()];
    let updated = state
        .update_skill(created.id.clone(), updated_input)
        .await
        .expect("update");
    assert_eq!(updated.id, created.id);
    assert_eq!(updated.name, "Rustacean");
    assert_eq!(updated.allowed_tools, vec!["read_file".to_owned()]);
    assert!(state
        .update_skill("missing".to_owned(), skill_input("X"))
        .await
        .expect_err("unknown skill")
        .contains("unknown skill"));
    assert!(state
        .delete_skill("missing".to_owned())
        .await
        .expect_err("unknown skill")
        .contains("unknown skill"));

    // The store round-trips through a reboot.
    drop(state);
    let rebooted = AppState::new_for_test(dir.path().join("settings.json"));
    let skills = rebooted.list_skills().await;
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].id, created.id);
    assert_eq!(skills[0].name, "Rustacean");
}

#[tokio::test]
async fn set_thread_skills_validates_ids_and_persists() {
    let (state, dir) = test_state();
    let project = dir.path().to_string_lossy().into_owned();
    let thread = state
        .create_thread(project, ProviderKind::Go, None)
        .await
        .expect("create");
    assert!(thread.skill_ids.is_empty());
    let skill = state
        .create_skill(skill_input("Rust"))
        .await
        .expect("skill");

    // Unknown ids fail, naming the bad id; valid refs are untouched.
    let err = state
        .set_thread_skills(thread.id.clone(), vec!["nope-1".to_owned()])
        .await
        .expect_err("unknown id rejected");
    assert!(err.contains("nope-1"), "{err}");
    let err = state
        .set_thread_skills("missing".to_owned(), vec![skill.id.clone()])
        .await
        .expect_err("unknown thread");
    assert!(err.contains("unknown thread"), "{err}");

    let info = state
        .set_thread_skills(thread.id.clone(), vec![skill.id.clone(), skill.id.clone()])
        .await
        .expect("set");
    assert_eq!(info.skill_ids, vec![skill.id.clone()]);
    assert_eq!(
        state.get_thread(&thread.id).await.expect("info").skill_ids,
        vec![skill.id.clone()]
    );
    // Clearing works too.
    let info = state
        .set_thread_skills(thread.id.clone(), Vec::new())
        .await
        .expect("clear");
    assert!(info.skill_ids.is_empty());

    // Reattach, then reboot: the reference survives.
    state
        .set_thread_skills(thread.id.clone(), vec![skill.id.clone()])
        .await
        .expect("reattach");
    drop(state);
    let rebooted = AppState::new_for_test(dir.path().join("settings.json"));
    assert_eq!(
        rebooted
            .get_thread(&thread.id)
            .await
            .expect("restored")
            .skill_ids,
        vec![skill.id]
    );
}

#[tokio::test]
async fn delete_skill_strips_references_everywhere() {
    let (state, dir) = test_state();
    let project = dir.path().to_string_lossy().into_owned();
    let thread = state
        .create_thread(project.clone(), ProviderKind::Go, None)
        .await
        .expect("create");
    let skill = state
        .create_skill(skill_input("Rust"))
        .await
        .expect("skill");
    state
        .set_thread_skills(thread.id.clone(), vec![skill.id.clone()])
        .await
        .expect("attach");
    let mut input = automation_input(&project);
    input.skill_ids = vec![skill.id.clone()];
    let automation = state.create_automation(input).await.expect("automation");
    assert_eq!(automation.skill_ids, vec![skill.id.clone()]);

    state.delete_skill(skill.id.clone()).await.expect("delete");
    assert!(state.list_skills().await.is_empty());
    assert!(state
        .get_thread(&thread.id)
        .await
        .expect("info")
        .skill_ids
        .is_empty());
    assert!(state
        .list_automations()
        .await
        .iter()
        .all(|automation| automation.skill_ids.is_empty()));
    // No dangling refs after a reboot either.
    drop(state);
    let rebooted = AppState::new_for_test(dir.path().join("settings.json"));
    assert!(rebooted
        .get_thread(&thread.id)
        .await
        .expect("restored")
        .skill_ids
        .is_empty());
    assert!(rebooted
        .list_automations()
        .await
        .iter()
        .all(|automation| automation.skill_ids.is_empty()));
}

#[tokio::test]
async fn automation_input_validation_rejects_bad_shapes() {
    let (state, dir) = test_state();
    let project = dir.path().to_string_lossy().into_owned();

    let mut empty_name = automation_input(&project);
    empty_name.name = "  ".to_owned();
    assert!(state
        .create_automation(empty_name)
        .await
        .expect_err("empty name")
        .contains("name must not be empty"));

    let mut empty_task = automation_input(&project);
    empty_task.task = String::new();
    assert!(state
        .create_automation(empty_task)
        .await
        .expect_err("empty task")
        .contains("task must not be empty"));

    for bad_interval in [0, -5] {
        let mut bad = automation_input(&project);
        bad.interval_mins = bad_interval;
        let err = state
            .create_automation(bad)
            .await
            .expect_err("bad interval rejected");
        assert!(err.contains("interval_mins"), "{err}");
    }

    let missing_path = dir
        .path()
        .join("does-not-exist")
        .to_string_lossy()
        .into_owned();
    let mut missing_root = automation_input(&missing_path);
    missing_root.task = "x".to_owned();
    assert!(state.create_automation(missing_root).await.is_err());

    let mut go_no_model = automation_input(&project);
    go_no_model.provider = ProviderKind::Go;
    go_no_model.model = String::new();
    assert!(state.create_automation(go_no_model).await.is_ok());

    let mut bad_skill = automation_input(&project);
    bad_skill.skill_ids = vec!["ghost".to_owned()];
    let err = state
        .create_automation(bad_skill)
        .await
        .expect_err("unknown skill rejected");
    assert!(err.contains("ghost"), "{err}");

    // A valid automation stores canonical roots and a future next run.
    let created = state
        .create_automation(automation_input(&project))
        .await
        .expect("create");
    assert_eq!(created.run_count, 0);
    assert!(created.last_run_at.is_none());
    assert!(created.enabled);
    let next: chrono::DateTime<chrono::Utc> =
        chrono::DateTime::parse_from_rfc3339(&created.next_run_at)
            .expect("rfc3339")
            .into();
    assert!(next > chrono::Utc::now());

    // Updates keep history but reschedule; unknown ids error clearly.
    let mut update = automation_input(&project);
    update.name = "Renamed".to_owned();
    update.interval_mins = 5;
    let updated = state
        .update_automation(created.id.clone(), update)
        .await
        .expect("update");
    assert_eq!(updated.id, created.id);
    assert_eq!(updated.name, "Renamed");
    assert_eq!(updated.interval_mins, 5);
    assert_eq!(updated.run_count, 0);
    assert!(state
        .update_automation("missing".to_owned(), automation_input(&project))
        .await
        .expect_err("unknown automation")
        .contains("unknown automation"));
    assert!(state
        .delete_automation("missing".to_owned())
        .await
        .expect_err("unknown automation")
        .contains("unknown automation"));
    state
        .delete_automation(created.id.clone())
        .await
        .expect("delete");
    assert_eq!(state.list_automations().await.len(), 1);
}

#[tokio::test]
async fn set_automation_enabled_flips_and_reschedules() {
    let (state, _dir) = test_state();
    let dir = tempfile::tempdir().expect("tempdir");
    let project = dir.path().to_string_lossy().into_owned();
    let created = state
        .create_automation(automation_input(&project))
        .await
        .expect("create");

    let disabled = state
        .set_automation_enabled(created.id.clone(), false)
        .await
        .expect("disable");
    assert!(!disabled.enabled);
    // Disabling keeps the existing schedule.
    assert_eq!(disabled.next_run_at, created.next_run_at);

    let before = chrono::Utc::now();
    let enabled = state
        .set_automation_enabled(created.id.clone(), true)
        .await
        .expect("enable");
    assert!(enabled.enabled);
    let next: chrono::DateTime<chrono::Utc> =
        chrono::DateTime::parse_from_rfc3339(&enabled.next_run_at)
            .expect("rfc3339")
            .into();
    let expected = before + chrono::Duration::minutes(60);
    assert!(
        (next - expected).num_seconds().abs() < 60,
        "enabling reschedules to now + interval: {next} vs {expected}"
    );
    assert!(state
        .set_automation_enabled("missing".to_owned(), true)
        .await
        .expect_err("unknown automation")
        .contains("unknown automation"));
}

#[tokio::test]
async fn tick_skips_when_globally_disabled_or_automation_disabled() {
    let (state, _dir) = test_state();
    let dir = tempfile::tempdir().expect("tempdir");
    let project = dir.path().to_string_lossy().into_owned();
    let (sink, _rx) = test_sink();

    // Disabled automation, due: no thread, schedule untouched.
    let mut input = automation_input(&project);
    input.enabled = false;
    let automation = state.create_automation(input).await.expect("create");
    state
        .set_automation_next_run_for_test(&automation.id, "2001-01-01T00:00:00Z".to_owned())
        .await
        .expect("force due");
    state.tick_automations_once(&sink).await;
    assert!(state
        .list_threads(project.clone())
        .await
        .expect("list")
        .is_empty());
    let untouched = state
        .list_automations()
        .await
        .into_iter()
        .find(|item| item.id == automation.id)
        .expect("still there");
    assert_eq!(untouched.next_run_at, "2001-01-01T00:00:00Z");
    assert_eq!(untouched.run_count, 0);

    // Enabled automation but global kill-switch off: same silence.
    let enabled = state
        .create_automation(automation_input(&project))
        .await
        .expect("create");
    state
        .set_automation_next_run_for_test(&enabled.id, "2001-01-01T00:00:00Z".to_owned())
        .await
        .expect("force due");
    state
        .update_settings(SettingsPatch {
            automations_enabled: Some(false),
            ..SettingsPatch::default()
        })
        .await
        .expect("kill switch off");
    state.tick_automations_once(&sink).await;
    assert!(state
        .list_threads(project.clone())
        .await
        .expect("list")
        .is_empty());
    assert!(state.list_review_items(None).await.is_empty());
}

#[tokio::test]
async fn tick_skips_a_vanished_project_root_without_advancing() {
    let (state, _dir) = test_state();
    let dir = tempfile::tempdir().expect("tempdir");
    let project_dir = dir.path().join("repo");
    std::fs::create_dir(&project_dir).expect("mkdir");
    let project = project_dir.to_string_lossy().into_owned();
    let (sink, _rx) = test_sink();

    let automation = state
        .create_automation(automation_input(&project))
        .await
        .expect("create");
    state
        .set_automation_next_run_for_test(&automation.id, "2001-01-01T00:00:00Z".to_owned())
        .await
        .expect("force due");
    std::fs::remove_dir_all(&project_dir).expect("remove root");

    state.tick_automations_once(&sink).await;
    let skipped = state
        .list_automations()
        .await
        .into_iter()
        .find(|item| item.id == automation.id)
        .expect("still there");
    assert_eq!(skipped.next_run_at, "2001-01-01T00:00:00Z");
    assert_eq!(skipped.run_count, 0);
    assert!(skipped.last_run_at.is_none());
    assert!(state.list_review_items(None).await.is_empty());
}

#[tokio::test]
async fn spawn_failure_advances_schedule_but_owes_no_review() {
    // No key and no custom base URL: the send path fails deterministically
    // without touching the network.
    let (state, _dir) = test_state();
    let dir = tempfile::tempdir().expect("tempdir");
    let project = dir.path().to_string_lossy().into_owned();
    let (sink, _rx) = test_sink();

    let automation = state
        .create_automation(automation_input(&project))
        .await
        .expect("create");
    state
        .set_automation_next_run_for_test(&automation.id, "2001-01-01T00:00:00Z".to_owned())
        .await
        .expect("force due");
    state.tick_automations_once(&sink).await;

    // The thread was still created (named for the automation)...
    let threads = state.list_threads(project).await.expect("list");
    assert_eq!(threads.len(), 1);
    assert!(
        threads[0].title.starts_with("Automation Nightly — "),
        "{}",
        threads[0].title
    );
    assert!(!threads[0].running);
    // ... the schedule advanced so the tick doesn't error-loop ...
    let advanced = state
        .list_automations()
        .await
        .into_iter()
        .find(|item| item.id == automation.id)
        .expect("still there");
    assert_eq!(advanced.run_count, 1);
    assert!(advanced.last_run_at.is_some());
    assert_ne!(advanced.next_run_at, "2001-01-01T00:00:00Z");
    let next: chrono::DateTime<chrono::Utc> =
        chrono::DateTime::parse_from_rfc3339(&advanced.next_run_at)
            .expect("rfc3339")
            .into();
    assert!(next > chrono::Utc::now());
    // ... and no review item exists, because no run completed.
    assert!(state.list_review_items(None).await.is_empty());
}

#[tokio::test]
async fn run_automation_now_rejects_unknown_and_vanished_roots() {
    let (state, _dir) = test_state();
    let dir = tempfile::tempdir().expect("tempdir");
    let project_dir = dir.path().join("repo");
    std::fs::create_dir(&project_dir).expect("mkdir");
    let (sink, _rx) = test_sink();

    let err = state
        .run_automation_now(sink.clone(), "missing".to_owned())
        .await
        .expect_err("unknown automation");
    assert!(err.contains("unknown automation"), "{err}");

    let project = project_dir.to_string_lossy().into_owned();
    let automation = state
        .create_automation(automation_input(&project))
        .await
        .expect("create");
    std::fs::remove_dir_all(&project_dir).expect("remove root");
    let err = state
        .run_automation_now(sink, automation.id.clone())
        .await
        .expect_err("vanished root");
    assert!(err.contains("no longer exists"), "{err}");
}

#[tokio::test]
async fn review_dismiss_and_continue_flow() {
    let (state, dir) = test_state();
    let project = dir.path().to_string_lossy().into_owned();
    let thread = state
        .create_thread(project, ProviderKind::Go, Some("m".to_owned()))
        .await
        .expect("create");
    // Seed one pending review through the store file, then reboot.
    let item = ReviewItem {
        id: "review-1".to_owned(),
        automation_id: "auto-1".to_owned(),
        thread_id: thread.id.clone(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        title: "Automation Nightly".to_owned(),
        summary: "done".to_owned(),
        status: ReviewStatus::Pending,
        source: Default::default(),
    };
    std::fs::write(
        dir.path().join("review_items.json"),
        serde_json::to_string_pretty(&vec![item]).expect("json"),
    )
    .expect("write");
    drop(state);
    let state = AppState::new_for_test(dir.path().join("settings.json"));

    assert_eq!(state.list_review_items(None).await.len(), 1);
    assert_eq!(
        state
            .list_review_items(Some(ReviewStatus::Pending))
            .await
            .len(),
        1
    );
    assert!(state
        .list_review_items(Some(ReviewStatus::Dismissed))
        .await
        .is_empty());

    let dismissed = state
        .dismiss_review_item("review-1".to_owned())
        .await
        .expect("dismiss");
    assert_eq!(dismissed.status, ReviewStatus::Dismissed);
    assert!(state
        .list_review_items(Some(ReviewStatus::Pending))
        .await
        .is_empty());
    assert!(state
        .dismiss_review_item("missing".to_owned())
        .await
        .expect_err("unknown review")
        .contains("unknown review"));

    // A second review exercises continue: it returns the thread.
    let item = ReviewItem {
        id: "review-2".to_owned(),
        automation_id: "auto-1".to_owned(),
        thread_id: thread.id.clone(),
        created_at: "2026-01-02T00:00:00Z".to_owned(),
        title: "Automation Nightly".to_owned(),
        summary: "done again".to_owned(),
        status: ReviewStatus::Pending,
        source: Default::default(),
    };
    std::fs::write(
        dir.path().join("review_items.json"),
        serde_json::to_string_pretty(&vec![dismissed, item]).expect("json"),
    )
    .expect("write");
    drop(state);
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let info = state
        .continue_review_item("review-2".to_owned())
        .await
        .expect("continue");
    assert_eq!(info.id, thread.id);
    assert_eq!(
        state
            .list_review_items(Some(ReviewStatus::Continued))
            .await
            .len(),
        1
    );
    assert!(state
        .continue_review_item("missing".to_owned())
        .await
        .expect_err("unknown review")
        .contains("unknown review"));
}

#[tokio::test]
async fn continue_review_item_keeps_status_when_thread_is_gone() {
    let (state, dir) = test_state();
    let project = dir.path().to_string_lossy().into_owned();
    let thread = state
        .create_thread(project, ProviderKind::Go, None)
        .await
        .expect("create");
    let item = ReviewItem {
        id: "review-1".to_owned(),
        automation_id: "auto-1".to_owned(),
        thread_id: thread.id.clone(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        title: "t".to_owned(),
        summary: "s".to_owned(),
        status: ReviewStatus::Pending,
        source: Default::default(),
    };
    std::fs::write(
        dir.path().join("review_items.json"),
        serde_json::to_string_pretty(&vec![item]).expect("json"),
    )
    .expect("write");
    drop(state);
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    state
        .discard_thread(thread.id.clone())
        .await
        .expect("discard");
    let err = state
        .continue_review_item("review-1".to_owned())
        .await
        .expect_err("thread gone");
    assert!(err.contains("unknown thread"), "{err}");
    // The status was NOT flipped.
    assert_eq!(
        state.list_review_items(None).await[0].status,
        ReviewStatus::Pending
    );
}

#[tokio::test]
async fn stores_survive_reboot() {
    let (state, dir) = test_state();
    let project = dir.path().to_string_lossy().into_owned();
    let skill = state
        .create_skill(skill_input("Rust"))
        .await
        .expect("skill");
    let automation = state
        .create_automation(automation_input(&project))
        .await
        .expect("automation");
    let thread = state
        .create_thread(project, ProviderKind::Go, Some("m".to_owned()))
        .await
        .expect("thread");
    let item = ReviewItem {
        id: "review-1".to_owned(),
        automation_id: automation.id.clone(),
        thread_id: thread.id.clone(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        title: "t".to_owned(),
        summary: "s".to_owned(),
        status: ReviewStatus::Pending,
        source: Default::default(),
    };
    std::fs::write(
        dir.path().join("review_items.json"),
        serde_json::to_string_pretty(&vec![item]).expect("json"),
    )
    .expect("write");
    drop(state);

    let rebooted = AppState::new_for_test(dir.path().join("settings.json"));
    assert_eq!(rebooted.list_skills().await.len(), 1);
    assert_eq!(rebooted.list_skills().await[0].id, skill.id);
    assert_eq!(rebooted.list_automations().await.len(), 1);
    assert_eq!(rebooted.list_automations().await[0].id, automation.id);
    assert_eq!(rebooted.list_review_items(None).await.len(), 1);
    assert!(rebooted.get_thread(&thread.id).await.is_ok());
}

#[tokio::test]
async fn boot_defers_missed_automations_without_herding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = dir.path().to_string_lossy().into_owned();
    let missed = Automation {
        id: "missed".to_owned(),
        name: "Missed".to_owned(),
        project_root: project.clone(),
        provider: ProviderKind::Go,
        model: "m".to_owned(),
        reasoning_effort: None,
        target_thread_id: None,
        skill_ids: Vec::new(),
        interval_mins: 30,
        schedule: None,
        task: "t".to_owned(),
        enabled: true,
        last_run_at: None,
        next_run_at: "2001-01-01T00:00:00Z".to_owned(),
        run_count: 4,
        origin: AutomationOrigin::User,
        watch: Default::default(),
        watch_state: Default::default(),
    };
    let future = Automation {
        id: "future".to_owned(),
        next_run_at: "2999-01-01T00:00:00Z".to_owned(),
        ..missed.clone()
    };
    std::fs::write(
        dir.path().join("automations.json"),
        serde_json::to_string_pretty(&vec![missed, future]).expect("json"),
    )
    .expect("write");

    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let automations = state.list_automations().await;
    let missed = automations
        .iter()
        .find(|item| item.id == "missed")
        .expect("missed");
    let next: chrono::DateTime<chrono::Utc> =
        chrono::DateTime::parse_from_rfc3339(&missed.next_run_at)
            .expect("rfc3339")
            .into();
    let delta = (next - chrono::Utc::now()).num_seconds();
    assert!(
        (30..=90).contains(&delta),
        "missed run deferred ~60s, got {delta}s"
    );
    assert_eq!(missed.run_count, 4);
    let future = automations
        .iter()
        .find(|item| item.id == "future")
        .expect("future");
    assert_eq!(future.next_run_at, "2999-01-01T00:00:00Z");
    assert!(state
        .reconcile_report()
        .iter()
        .any(|note| note.contains("deferred 1 missed automation")));
    // The deferral was persisted back, so a second boot defers from the
    // new stamp rather than stacking (still ~60s out, not firing).
    drop(state);
    let rebooted = AppState::new_for_test(dir.path().join("settings.json"));
    let missed = rebooted
        .list_automations()
        .await
        .into_iter()
        .find(|item| item.id == "missed")
        .expect("missed");
    let next: chrono::DateTime<chrono::Utc> =
        chrono::DateTime::parse_from_rfc3339(&missed.next_run_at)
            .expect("rfc3339")
            .into();
    assert!(next > chrono::Utc::now());
}

#[tokio::test]
async fn old_settings_file_without_kill_switch_loads_enabled() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("settings.json"),
        r#"{"theme":"dark","default_provider":"openai","default_model":"gpt-x","max_turns":7}"#,
    )
    .expect("write");
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let settings = state.get_settings().await;
    assert!(settings.automations_enabled);
    assert!(!settings.onboarded);
}

#[tokio::test]
async fn corrupt_stores_boot_empty_and_report() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("skills.json"), "{ nope").expect("write");
    std::fs::write(dir.path().join("automations.json"), "[ broken").expect("write");
    std::fs::write(dir.path().join("review_items.json"), "zzz").expect("write");
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    assert!(state.list_skills().await.is_empty());
    assert!(state.list_automations().await.is_empty());
    assert!(state.list_review_items(None).await.is_empty());
    let report = state.reconcile_report();
    assert_eq!(report.len(), 3, "{report:?}");
}

#[test]
fn failed_commands_log_a_category_never_the_message() {
    let (state, dir) = test_state();
    for (message, category) in [
        ("unknown server method 'resource_usage'", "unknown_method"),
        (
            "Server response exceeds 8 MiB; use a bounded or paged API",
            "too_large",
        ),
        (
            "invalid threadId: invalid type: null, expected a string",
            "invalid_request",
        ),
        ("Go model list timed out", "timeout"),
        ("unknown thread 1234", "not_found"),
        (
            "The system cannot find the path specified. (os error 3)",
            "not_found",
        ),
        (
            "plugin store revision changed; reload and try again",
            "conflict",
        ),
        (
            "permission denied: /Users/someone/private/notes.txt",
            "denied",
        ),
        (
            "HTTP Status Error (503): OpenCode Go request failed",
            "network",
        ),
        ("Missing go API key", "credentials"),
        (
            "something unexpected about /Users/someone/secret-project",
            "other",
        ),
    ] {
        assert_eq!(failure_category(message), category, "{message}");
        state.record_error("cmd", message.to_owned());
    }
    let log = std::fs::read_dir(dir.path().join("logs"))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<String>();
    let failed: Vec<serde_json::Value> = log
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|record| record["event"] == "command_failed")
        .collect();
    assert_eq!(failed.len(), 11);
    assert_eq!(failed[0]["details"]["category"], "unknown_method");
    assert_eq!(failed[10]["details"]["category"], "other");
    // The messages (paths, user text) stay out of the log; the ring keeps them.
    assert!(!log.contains("someone"), "{log}");
    assert!(state
        .recent_errors()
        .iter()
        .any(|entry| entry.message.contains("secret-project")));
}

#[test]
fn diagnostics_ring_evicts_oldest_at_cap() {
    let (state, _dir) = test_state();
    assert!(state.recent_errors().is_empty());
    for index in 0..(MAX_RECENT_ERRORS + 5) {
        state.record_error("cmd", format!("error-{index}"));
    }
    let errors = state.recent_errors();
    assert_eq!(errors.len(), MAX_RECENT_ERRORS);
    // Oldest-first: the first five entries were evicted.
    assert_eq!(errors.first().expect("first").message, "error-5");
    assert_eq!(
        errors.last().expect("last").message,
        format!("error-{}", MAX_RECENT_ERRORS + 4)
    );
    for (position, entry) in errors.iter().enumerate() {
        assert_eq!(entry.message, format!("error-{}", position + 5));
    }
}

#[tokio::test]
async fn diagnostics_snapshot_reports_version_os_settings_and_errors() {
    let (state, _dir) = test_state();
    // A genuine handler failure, recorded exactly as the `open_project`
    // command wrapper records it (see `record_cmd!` in commands.rs).
    let err = state
        .open_project("/definitely/not/a/themis/dir".to_owned())
        .await
        .expect_err("missing dir fails");
    state.record_error("open_project", err);
    let diagnostics = state.get_diagnostics().await;
    assert_eq!(diagnostics.app_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(diagnostics.os, std::env::consts::OS);
    assert_eq!(diagnostics.settings, state.get_settings().await);
    assert_eq!(diagnostics.recent_errors.len(), 1);
    let entry = &diagnostics.recent_errors[0];
    assert_eq!(entry.command, "open_project");
    assert!(!entry.message.is_empty());
    // The timestamp parses as RFC3339.
    let parsed: DateTime<Utc> = entry.at.parse().expect("RFC3339 timestamp");
    assert!(parsed <= Utc::now());
}
