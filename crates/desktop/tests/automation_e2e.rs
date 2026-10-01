//! Automation engine tests: skilled runs, the scheduler tick, review items,
//! and the never-auto-merge guarantee. The real [`AppState`] handlers run
//! headless against a scripted mock LLM (wiremock); no network beyond
//! localhost. Tests needing `git` skip gracefully when the binary is missing.

use std::sync::Arc;
use std::time::Duration;

mod common;
use common::{git_available, init_repo, mount_scripted_llm, use_mock_llm};

use serde_json::{json, Value};
use themis_desktop::sink::{ChannelSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{
    ApprovalDecision, AutomationInput, ProviderKind, ReviewStatus, SettingsPatch, SkillInput,
    ThreadEvent,
};
use tokio::sync::mpsc::UnboundedReceiver;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Overall deadline for an LLM-driven run.
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

fn test_state() -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    (state, dir)
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

fn automation_input(project_root: &str, task: &str) -> AutomationInput {
    AutomationInput {
        name: "Nightly".to_owned(),
        project_root: project_root.to_owned(),
        provider: ProviderKind::Go,
        model: "test-model".to_owned(),
        reasoning_effort: None,
        target_thread_id: None,
        skill_ids: Vec::new(),
        interval_mins: 60,
        task: task.to_owned(),
        enabled: true,
    }
}

/// The model always answers with final text (no tool calls).
async fn mount_final_text_llm(answer: &str) -> MockServer {
    let server = MockServer::start().await;
    let body = json!({
        "choices": [{
            "message": {"role": "assistant", "content": answer},
            "finish_reason": "stop"
        }]
    });
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    server
}

struct DrivenRun {
    thread_id: String,
    run_id: String,
    events: Vec<ThreadEvent>,
    reviews_seen: usize,
}

/// Drives events to the first terminal thread event, answering every
/// approval dialog with `AllowOnce`. Returns the run it observed.
async fn drive_one_run(state: &AppState, rx: &mut UnboundedReceiver<TestEvent>) -> DrivenRun {
    let mut events = Vec::new();
    let mut reviews_seen = 0;
    let (thread_id, run_id) = tokio::time::timeout(RUN_TIMEOUT, async {
        loop {
            match rx.recv().await.expect("event stream open") {
                TestEvent::Approval(request) => {
                    state
                        .approve_action(
                            request.thread_id.clone(),
                            request.approval_id.clone(),
                            ApprovalDecision::Once,
                        )
                        .await
                        .expect("approve");
                }
                TestEvent::Review(_) => {
                    reviews_seen += 1;
                }
                TestEvent::Thread(envelope) => {
                    let terminal = matches!(
                        envelope.event,
                        ThreadEvent::Finished { .. } | ThreadEvent::Failed { .. }
                    );
                    events.push(envelope.event);
                    if terminal {
                        return (envelope.thread_id, envelope.run_id);
                    }
                }
            }
        }
    })
    .await
    .expect("run finished before the deadline");
    DrivenRun {
        thread_id,
        run_id,
        events,
        reviews_seen,
    }
}

#[tokio::test]
async fn skilled_run_filters_tools_composes_instructions_and_materializes() {
    if !git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    const MARKER: &str = "SKILL-MARKER-UNIQUE-42";
    let (_repo_temp, root) = init_repo();
    let (state, _settings_temp) = test_state();
    let server = mount_final_text_llm("done, skillfully").await;
    use_mock_llm(&state, &server).await;

    let mut input = skill_input("Reader");
    input.instructions = format!("{MARKER}: always read before writing.");
    input.allowed_tools = vec!["read_file".to_owned()];
    input.scripts = vec![themis_desktop::types::SkillScript {
        name: "helper.sh".to_owned(),
        content: "echo help".to_owned(),
    }];
    let skill = state.create_skill(input).await.expect("create skill");

    let thread = state
        .create_thread(
            root.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".to_owned()),
        )
        .await
        .expect("create thread");
    state
        .set_thread_skills(thread.id.clone(), vec![skill.id.clone()])
        .await
        .expect("attach skill");

    let (sink, mut rx) = ChannelSink::channel();
    let handle = state
        .send_message(
            Arc::new(sink),
            thread.id.clone(),
            "read the README".to_owned(),
        )
        .await
        .expect("send");
    let run = drive_one_run(&state, &mut rx).await;
    assert_eq!(run.thread_id, thread.id);
    assert_eq!(run.run_id, handle.run_id);
    assert!(matches!(
        run.events.last(),
        Some(ThreadEvent::Finished { result }) if result == "done, skillfully"
    ));

    // The tools schema sent to the model carries only the granted tool...
    let requests = server.received_requests().await.expect("captured");
    assert!(!requests.is_empty());
    let first: Value = serde_json::from_slice(&requests[0].body).expect("json body");
    let tool_names: Vec<String> = first["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| {
            tool["function"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(tool_names, vec!["read_file".to_owned()]);
    // ... and the skill instructions reached the model in the prompt.
    let body_text = serde_json::to_string(&first).expect("json");
    assert!(body_text.contains("## Skill: Reader"), "{body_text}");
    assert!(body_text.contains(MARKER), "{body_text}");

    // The skill script was materialized under the run workroot.
    let worktree = root.clone();
    let script_path = worktree
        .join(".themis")
        .join("skills")
        .join(&skill.id)
        .join("helper.sh");
    assert_eq!(
        std::fs::read_to_string(&script_path).expect("script materialized"),
        "echo help"
    );
}

#[tokio::test]
async fn set_thread_skills_rejects_busy_threads() {
    if !git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    let (_repo_temp, root) = init_repo();
    let (state, _settings_temp) = test_state();
    let server = mount_scripted_llm().await;
    use_mock_llm(&state, &server).await;
    let skill = state
        .create_skill(skill_input("Rust"))
        .await
        .expect("skill");
    let thread = state
        .create_thread(
            root.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".to_owned()),
        )
        .await
        .expect("create thread");

    let (sink, mut rx) = ChannelSink::channel();
    state
        .send_message(
            Arc::new(sink),
            thread.id.clone(),
            "write hello.txt".to_owned(),
        )
        .await
        .expect("send spawns");

    // While the run is alive (blocked on its approval dialog), skills are locked.
    let busy = loop {
        // Drain one event: the run starts before it asks for approval.
        match tokio::time::timeout(RUN_TIMEOUT, rx.recv())
            .await
            .expect("event before deadline")
            .expect("stream open")
        {
            TestEvent::Approval(request) => {
                let busy = state
                    .set_thread_skills(thread.id.clone(), vec![skill.id.clone()])
                    .await
                    .expect_err("busy reject");
                assert!(busy.contains("busy"), "{busy}");
                // Answer the dialog so the run can finish.
                state
                    .approve_action(
                        thread.id.clone(),
                        request.approval_id.clone(),
                        ApprovalDecision::Once,
                    )
                    .await
                    .expect("approve");
                break busy;
            }
            TestEvent::Thread(_) => continue,
            TestEvent::Review(item) => panic!("unexpected review: {item:?}"),
        }
    };
    assert!(busy.contains("busy"));
    let run = drive_one_run(&state, &mut rx).await;
    assert!(matches!(
        run.events.last(),
        Some(ThreadEvent::Finished { .. })
    ));

    // Idle again: attaching works.
    let info = state
        .set_thread_skills(thread.id.clone(), vec![skill.id.clone()])
        .await
        .expect("attach when idle");
    assert_eq!(info.skill_ids, vec![skill.id]);
}

#[tokio::test]
async fn automation_tick_creates_thread_run_and_review() {
    if !git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    let (_repo_temp, root) = init_repo();
    let (state, settings_temp) = test_state();
    let server = mount_final_text_llm("automation result summary").await;
    use_mock_llm(&state, &server).await;

    let automation = state
        .create_automation(automation_input(
            &root.to_string_lossy(),
            "Do the nightly thing.",
        ))
        .await
        .expect("create automation");
    state
        .set_automation_next_run_for_test(&automation.id, "2001-01-01T00:00:00Z".to_owned())
        .await
        .expect("force due");

    let (sink, mut rx) = ChannelSink::channel();
    let sink: Arc<dyn themis_desktop::sink::EventSink> = Arc::new(sink);
    state.tick_automations_once(&sink).await;
    let run = drive_one_run(&state, &mut rx).await;
    assert!(matches!(
        run.events.last(),
        Some(ThreadEvent::Finished { .. })
    ));
    // The review event was emitted on completion.
    assert_eq!(run.reviews_seen, 1);

    // The automation thread is titled and carries the run.
    let threads = state
        .list_threads(root.to_string_lossy().into_owned())
        .await
        .expect("list");
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].id, run.thread_id);
    assert!(
        threads[0].title.starts_with("Automation Nightly — "),
        "{}",
        threads[0].title
    );
    assert!(!threads[0].running);

    // The review item landed with the result summary...
    let reviews = state.list_review_items(None).await;
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].automation_id, automation.id);
    assert_eq!(reviews[0].thread_id, run.thread_id);
    assert_eq!(reviews[0].summary, "automation result summary");
    assert_eq!(reviews[0].status, ReviewStatus::Pending);
    assert_eq!(reviews[0].title, threads[0].title);
    // ... and the schedule advanced.
    let advanced = state
        .list_automations()
        .await
        .into_iter()
        .find(|item| item.id == automation.id)
        .expect("automation");
    assert_eq!(advanced.run_count, 1);
    assert!(advanced.last_run_at.is_some());
    let next: chrono::DateTime<chrono::Utc> =
        chrono::DateTime::parse_from_rfc3339(&advanced.next_run_at)
            .expect("rfc3339")
            .into();
    assert!(next > chrono::Utc::now());

    // Reviews and schedule history survive a reboot.
    drop(state);
    let rebooted = AppState::new_for_test(settings_temp.path().join("settings.json"));
    let reviews = rebooted.list_review_items(None).await;
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].summary, "automation result summary");
    let automation = rebooted
        .list_automations()
        .await
        .into_iter()
        .find(|item| item.id == automation.id)
        .expect("automation");
    assert_eq!(automation.run_count, 1);
    assert!(automation.last_run_at.is_some());
}

#[tokio::test]
async fn continuing_automation_reuses_thread_and_current_settings() {
    if !git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    let (_repo_temp, root) = init_repo();
    let (state, _settings_temp) = test_state();
    let server = mount_final_text_llm("heartbeat complete").await;
    use_mock_llm(&state, &server).await;
    let thread = state
        .create_thread(
            root.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("first-model".to_owned()),
        )
        .await
        .expect("thread");
    let mut input = automation_input(&root.to_string_lossy(), "Check this thread.");
    input.target_thread_id = Some(thread.id.clone());
    let automation = state.create_automation(input).await.expect("automation");
    let (sink, mut rx) = ChannelSink::channel();
    let sink: Arc<dyn themis_desktop::sink::EventSink> = Arc::new(sink);
    let first = state
        .run_automation_now(Arc::clone(&sink), automation.id.clone())
        .await
        .expect("first heartbeat");
    assert_eq!(first.thread_id, thread.id);
    drive_one_run(&state, &mut rx).await;

    let skill = state
        .create_skill(skill_input("Current skill"))
        .await
        .expect("skill");
    state
        .set_thread_skills(thread.id.clone(), vec![skill.id.clone()])
        .await
        .expect("attach skill");
    state
        .set_provider(
            thread.id.clone(),
            ProviderKind::Go,
            Some("second-model".to_owned()),
        )
        .await
        .expect("change model");
    state
        .set_automation_next_run_for_test(&automation.id, "2001-01-01T00:00:00Z".to_owned())
        .await
        .expect("due");
    state.tick_automations_once(&sink).await;
    let second = drive_one_run(&state, &mut rx).await;
    assert_eq!(second.thread_id, thread.id);
    let threads = state
        .list_threads(root.to_string_lossy().into_owned())
        .await
        .expect("threads");
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].model, "second-model");
    assert_eq!(threads[0].skill_ids, vec![skill.id]);
    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 2);
    let first: Value = serde_json::from_slice(&requests[0].body).expect("first body");
    let second: Value = serde_json::from_slice(&requests[1].body).expect("second body");
    assert_eq!(first["model"], "first-model");
    assert_eq!(second["model"], "second-model");
    assert!(
        !second.to_string().contains("## Skill: Current skill"),
        "Automation capabilities come only from its prompt"
    );
    assert!(second.to_string().contains("heartbeat complete"));
    assert_eq!(state.list_review_items(None).await.len(), 2);
    assert_eq!(state.list_automations().await[0].run_count, 2);
}

#[tokio::test]
async fn automation_runs_use_shared_checkout() {
    if !git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    let (_repo_temp, root) = init_repo();
    let (state, _settings_temp) = test_state();
    let server = mount_scripted_llm().await;
    use_mock_llm(&state, &server).await;

    let automation = state
        .create_automation(automation_input(
            &root.to_string_lossy(),
            "Write hello.txt.",
        ))
        .await
        .expect("create automation");
    state
        .set_automation_next_run_for_test(&automation.id, "2001-01-01T00:00:00Z".to_owned())
        .await
        .expect("force due");

    let (sink, mut rx) = ChannelSink::channel();
    let sink: Arc<dyn themis_desktop::sink::EventSink> = Arc::new(sink);
    state.tick_automations_once(&sink).await;
    let run = drive_one_run(&state, &mut rx).await;
    assert!(matches!(
        run.events.last(),
        Some(ThreadEvent::Finished { .. })
    ));

    // Automation threads use the same checkout as interactive threads.
    let thread = state.get_thread(&run.thread_id).await.expect("thread");
    assert!(thread.worktree_path.is_none());
    assert!(thread.branch.is_none());
    assert_eq!(
        std::fs::read_to_string(root.join("hello.txt")).unwrap(),
        "hi from agent\n"
    );
    // The diff still lists the change for human review.
    let diff = state.list_diff(run.thread_id.clone()).await.expect("diff");
    assert!(diff.available);
    assert!(diff.files.iter().any(|file| file.path == "hello.txt"));
    // And the review item points at the thread.
    let reviews = state.list_review_items(None).await;
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].thread_id, run.thread_id);
}

#[tokio::test]
async fn saturated_gate_skips_without_advancing() {
    if !git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    let (_repo_temp, root) = init_repo();
    let (state, _settings_temp) = test_state();
    let server = mount_scripted_llm().await;
    use_mock_llm(&state, &server).await;
    state
        .update_settings(SettingsPatch {
            concurrency_limit: Some(1),
            ..SettingsPatch::default()
        })
        .await
        .expect("limit 1");

    // Run A holds the only permit, blocked on its approval dialog.
    let thread_a = state
        .create_thread(
            root.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".to_owned()),
        )
        .await
        .expect("create thread");
    let (sink_a, mut rx_a) = ChannelSink::channel();
    state
        .send_message(
            Arc::new(sink_a),
            thread_a.id.clone(),
            "write hello.txt".to_owned(),
        )
        .await
        .expect("send spawns");
    let approval_id = wait_for_approval_id(&mut rx_a).await;

    let automation = state
        .create_automation(automation_input(
            &root.to_string_lossy(),
            "Do the nightly thing.",
        ))
        .await
        .expect("create automation");
    state
        .set_automation_next_run_for_test(&automation.id, "2001-01-01T00:00:00Z".to_owned())
        .await
        .expect("force due");

    // The tick skips: no thread minted, schedule untouched, no review.
    let (tick_sink, _tick_rx) = ChannelSink::channel();
    let tick_sink: Arc<dyn themis_desktop::sink::EventSink> = Arc::new(tick_sink);
    state.tick_automations_once(&tick_sink).await;
    let threads = state
        .list_threads(root.to_string_lossy().into_owned())
        .await
        .expect("list");
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].id, thread_a.id);
    let skipped = state
        .list_automations()
        .await
        .into_iter()
        .find(|item| item.id == automation.id)
        .expect("automation");
    assert_eq!(skipped.next_run_at, "2001-01-01T00:00:00Z");
    assert_eq!(skipped.run_count, 0);
    assert!(state.list_review_items(None).await.is_empty());

    // Manual triggers respect the gate too.
    let err = state
        .run_automation_now(tick_sink, automation.id.clone())
        .await
        .expect_err("gate respected");
    assert!(err.contains("concurrency limit reached"), "{err}");

    // Cleanup: answer run A's dialog and let it finish.
    state
        .approve_action(thread_a.id.clone(), approval_id, ApprovalDecision::Once)
        .await
        .expect("approve");
    let run = drive_one_run(&state, &mut rx_a).await;
    assert!(matches!(
        run.events.last(),
        Some(ThreadEvent::Finished { .. })
    ));
}

/// Waits for the next approval dialog, dropping earlier thread events.
async fn wait_for_approval_id(rx: &mut UnboundedReceiver<TestEvent>) -> String {
    tokio::time::timeout(RUN_TIMEOUT, async {
        loop {
            match rx.recv().await.expect("event stream open") {
                TestEvent::Approval(request) => return request.approval_id,
                TestEvent::Thread(_) => continue,
                TestEvent::Review(item) => panic!("unexpected review: {item:?}"),
            }
        }
    })
    .await
    .expect("approval arrived before the deadline")
}

#[tokio::test]
async fn run_automation_now_works_while_disabled() {
    if !git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    let (_repo_temp, root) = init_repo();
    let (state, _settings_temp) = test_state();
    let server = mount_final_text_llm("manual run summary").await;
    use_mock_llm(&state, &server).await;

    // Disabled automation + global kill-switch off: the scheduler would stay
    // silent, but a manual trigger still fires.
    let mut input = automation_input(&root.to_string_lossy(), "Do the thing now.");
    input.enabled = false;
    let automation = state.create_automation(input).await.expect("create");
    state
        .update_settings(SettingsPatch {
            automations_enabled: Some(false),
            ..SettingsPatch::default()
        })
        .await
        .expect("kill switch off");

    let (sink, mut rx) = ChannelSink::channel();
    let fired = state
        .run_automation_now(Arc::new(sink), automation.id.clone())
        .await
        .expect("manual trigger fires while disabled");
    assert_eq!(fired.automation_id, automation.id);
    let run = drive_one_run(&state, &mut rx).await;
    assert_eq!(run.thread_id, fired.thread_id);
    assert_eq!(run.run_id, fired.run_id);
    assert!(matches!(
        run.events.last(),
        Some(ThreadEvent::Finished { .. })
    ));

    let reviews = state.list_review_items(None).await;
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].automation_id, automation.id);
    assert_eq!(reviews[0].thread_id, fired.thread_id);
    assert_eq!(reviews[0].summary, "manual run summary");
    let advanced = state
        .list_automations()
        .await
        .into_iter()
        .find(|item| item.id == automation.id)
        .expect("automation");
    assert_eq!(advanced.run_count, 1);
}

#[tokio::test]
async fn creator_prompt_reaches_llm_with_management_tools_and_verifier() {
    let (_repo_temp, root) = init_repo();
    let (state, _settings_temp) = test_state();
    let server = mount_final_text_llm("creator ready").await;
    use_mock_llm(&state, &server).await;
    let thread = state
        .create_thread(
            root.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("fixture".into()),
        )
        .await
        .unwrap();
    let (sink, mut rx) = ChannelSink::channel();
    state
        .send_message(
            Arc::new(sink),
            thread.id,
            "[[skill:create-skill]] Design a local review skill".into(),
        )
        .await
        .unwrap();
    drive_one_run(&state, &mut rx).await;
    let requests = server.received_requests().await.unwrap();
    let request: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert!(request
        .to_string()
        .contains("python3 .themis/skills/create-skill/verify.py"));
    let names: Vec<_> = request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    assert!(names.contains(&"save_skill"));
    assert!(names.contains(&"list_plugins"));
    assert!(root.join(".themis/skills/create-skill/verify.py").is_file());
}
