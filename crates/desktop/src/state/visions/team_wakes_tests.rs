// Team wakes (Task 4 review): questions a worker leaves open, a worker's waiting work after a
// team pause, a chief that is deleted or paused, and a worker's own pass.

use themis_core::visions::Decision;

use super::*;
use crate::state::questions::NewQuestion;

fn ask(state: &AppState, thread: &str, text: &str) -> String {
    let asked = NewQuestion {
        question: text.into(),
        ..NewQuestion::default()
    };
    state
        .ask_question(sink().as_ref(), thread, "run", asked)
        .unwrap()
        .id
}

fn passed_on(state: &AppState, channel: &str) -> Vec<serde_json::Value> {
    state
        .channel_page(channel, None, 1 << 20)
        .unwrap()
        .entries
        .into_iter()
        .map(|line| serde_json::to_value(line.entry).unwrap())
        .filter(|entry| entry["type"] == "passed_on")
        .collect()
}

fn finished() -> ThreadEvent {
    ThreadEvent::Finished {
        model: None,
        result: "ok".into(),
    }
}

/// Wren leads, Ada works. A question in Ada's chat that Ada's turn ended without answering or
/// passing on reaches the person, under Wren.
#[test]
fn a_question_its_worker_left_open_reaches_the_person() {
    let s = setup(true);
    own(&s.state, "ada-work", &s.ada, &s.channel, Hold::None);
    let chat = Owner {
        thread_id: "ada-chat".into(),
        vision_id: s.ada.clone(),
        channel_id: s.channel.clone(),
        kind: OwnedKind::VisionChat,
        hold: Hold::None,
    };
    s.state.owner_set(chat.clone()).unwrap();
    let question = ask(&s.state, "ada-work", "Which license?");
    // Ada's turn starts after the question was asked (admitted as a run would be; a unit test
    // has no runtime to spawn it), and ends with it still open.
    let started = |seq| {
        s.state
            .visions_cache()
            .wakes
            .admit(&s.ada, &s.channel, Some(WakeReason::Tasked(seq)), 10)
    };
    assert!(matches!(started(1), Admit::Start(_)));
    s.state.visions_after_run(&sink(), "ada-chat", &finished());
    let passed = passed_on(&s.state, &s.channel);
    assert_eq!(passed.len(), 1, "{passed:?}");
    assert_eq!(passed[0]["question_id"], question.as_str());
    assert_eq!(passed[0]["vision_id"], s.wren.as_str());
    // Not again at the next turn's end.
    assert!(matches!(started(2), Admit::Start(_)));
    s.state.visions_after_run(&sink(), "ada-chat", &finished());
    assert_eq!(passed_on(&s.state, &s.channel).len(), 1);
}

/// A worker's own pass keeps the decision it named.
#[test]
fn a_workers_pass_keeps_its_decision() {
    let s = setup(true);
    own(&s.state, "ada-work", &s.ada, &s.channel, Hold::None);
    let question = ask(&s.state, "ada-work", "Drop Windows?");
    let scope = themis_core::agent_cli::VisionScope {
        vision_id: s.ada.clone(),
        channel_id: s.channel.clone(),
        project_root: std::path::PathBuf::new(),
        decides: Vec::new(),
        chief: false,
    };
    s.state
        .visions_pass_question(&scope, "ada-work", &question, Decision::Scope)
        .unwrap();
    let passed = passed_on(&s.state, &s.channel);
    assert_eq!(passed[0]["decision"], "scope");
    assert_eq!(passed[0]["vision_id"], s.wren.as_str());
}

/// After the team's pause, the person's post restarts the worker's waiting work too.
#[test]
fn the_persons_post_restarts_a_workers_waiting_work() {
    let s = setup(true);
    let runtime = s.state.inner.settings.path().with_file_name("runtime.jsonc");
    std::fs::write(&runtime, r#"{"visions": {"max_wakes": 1}}"#).unwrap();
    let wake = |vision: &str, reason| s.state.visions_wake(&sink(), &s.channel, vision, reason);
    assert_eq!(wake(&s.wren, WakeReason::Mentioned), "started");
    assert_eq!(wake(&s.ada, WakeReason::Tasked(1)), "limit");
    std::fs::write(&runtime, r#"{"visions": {"max_wakes": 5}}"#).unwrap();
    s.state.visions_post(&sink(), &s.channel, "carry on").unwrap();
    assert!(started(&s.state).contains(&s.ada), "{:?}", started(&s.state));
}

/// Deleting the chief hands the channel to its next member, whom the person's post wakes.
#[test]
fn a_deleted_chief_hands_the_channel_on() {
    let s = setup(true);
    s.state.visions_delete(&sink(), &s.wren).unwrap();
    let channel = s.state.channel_get(&s.channel).unwrap();
    assert_eq!(channel.chief, s.ada);
    s.state.visions_post(&sink(), &s.channel, "hello").unwrap();
    assert_eq!(started(&s.state), vec![s.ada.clone()]);
}

/// A worker its chief paused is not paused once it leads the channel, whether the chief was
/// deleted or the person made it chief in the channel's details.
#[test]
fn a_paused_worker_that_becomes_chief_is_not_paused() {
    let s = setup(true);
    s.state.visions_team_pause(&s.channel, &s.ada, true).unwrap();
    s.state.visions_delete(&sink(), &s.wren).unwrap();
    let channel = s.state.channel_get(&s.channel).unwrap();
    assert_eq!(channel.chief, s.ada);
    assert!(channel.paused_workers.is_empty(), "{:?}", channel.paused_workers);
    s.state.visions_post(&sink(), &s.channel, "hello").unwrap();
    assert_eq!(started(&s.state), vec![s.ada.clone()]);

    let s = setup(true);
    s.state.visions_team_pause(&s.channel, &s.ada, true).unwrap();
    let channel = s.state.channel_get(&s.channel).unwrap();
    let revision = channel.revision.clone();
    let made_chief = Channel {
        chief: s.ada.clone(),
        ..channel
    };
    let saved = s.state.channel_save(made_chief, Some(&revision)).unwrap();
    assert!(saved.paused_workers.is_empty(), "{:?}", saved.paused_workers);
}

/// A worker's approval box waits in the worker's thread: it wakes no one, so it spends none of
/// the team's wakes.
#[test]
fn a_worker_approval_wakes_no_one() {
    let s = setup(true);
    own(&s.state, "ada-work", &s.ada, &s.channel, Hold::None);
    s.state.visions_approval_waiting(&sink(), "ada-work", "ap-1");
    assert!(started(&s.state).is_empty(), "{:?}", started(&s.state));
    // Not while the chief is paused either.
    s.state.visions_pause(&s.wren, true).unwrap();
    s.state.visions_approval_waiting(&sink(), "ada-work", "ap-2");
    assert!(started(&s.state).is_empty(), "{:?}", started(&s.state));
}

/// A permission question in a worker's chat goes straight to the person (passed on under the
/// chief), and wakes no one.
#[test]
fn a_worker_permission_question_wakes_no_one() {
    let s = setup(true);
    own(&s.state, "ada-work", &s.ada, &s.channel, Hold::None);
    let announced = Changes::default();
    s.state.set_scheduler_sink(Arc::new(announced.clone()));
    let asked = NewQuestion {
        question: "Run this outside the sandbox?".into(),
        permission: true,
        ..NewQuestion::default()
    };
    let question = s
        .state
        .ask_question(sink().as_ref(), "ada-work", "run", asked)
        .unwrap()
        .id;
    s.state
        .visions_question_asked("ada-work", std::slice::from_ref(&question));
    assert!(started(&s.state).is_empty(), "{:?}", started(&s.state));
    let passed = passed_on(&s.state, &s.channel);
    assert_eq!(passed.len(), 1, "{passed:?}");
    assert_eq!(passed[0]["question_id"], question.as_str());
    // The open channel is told, so the card shows where the person can answer it.
    assert!(announced.channels().contains(&s.channel), "{:?}", announced.channels());
}

/// A sink that keeps the channels `visions-changed` named.
#[derive(Clone, Default)]
struct Changes(Arc<std::sync::Mutex<Vec<Option<String>>>>);

impl Changes {
    fn channels(&self) -> Vec<String> {
        self.0.lock().unwrap().iter().flatten().cloned().collect()
    }
}

impl EventSink for Changes {
    fn emit_thread_event(&self, _: &crate::types::ThreadEventEnvelope) {}
    fn emit_approval_request(&self, _: &crate::types::ApprovalRequest) {}
    fn emit_approval_expired(&self, _: &crate::types::ApprovalExpired) {}
    fn emit_review_item(&self, _: &crate::types::ReviewItem) {}
    fn emit_thread_changed(&self, _: &crate::types::ThreadChange) {}
    fn emit_settings_changed(&self, _: &crate::types::SettingsChange) {}
    fn emit_visions_changed(&self, change: &crate::types::VisionsChange) {
        self.0.lock().unwrap().push(change.channel_id.clone());
    }
}

/// Once a question went to the person, the worker may no longer answer it.
#[tokio::test]
async fn a_worker_cannot_answer_a_question_passed_to_the_person() {
    let s = setup(true);
    own(&s.state, "ada-work", &s.ada, &s.channel, Hold::None);
    let question = ask(&s.state, "ada-work", "Which license?");
    let scope = themis_core::agent_cli::VisionScope {
        vision_id: s.ada.clone(),
        channel_id: s.channel.clone(),
        project_root: std::path::PathBuf::new(),
        decides: vec![Decision::Wording],
        chief: false,
    };
    s.state
        .visions_pass_question(&scope, "ada-work", &question, Decision::Scope)
        .unwrap();
    let answered = s
        .state
        .visions_answer(
            sink(),
            &scope,
            "ada-work".into(),
            vec![(question, "MIT".into())],
            (Decision::Wording, "fine".into()),
        )
        .await;
    assert_eq!(answered.unwrap_err(), themis_core::prompts::AGENT_CLI_QUESTION_PASSED_ON);
}

/// A question no run may take (here its worker is paused) reaches the person at once, under
/// the chief.
#[test]
fn a_question_no_agent_may_answer_reaches_the_person_anyway() {
    let s = setup(true);
    own(&s.state, "ada-work", &s.ada, &s.channel, Hold::None);
    let question = ask(&s.state, "ada-work", "Which license?");
    // As in the app, a server is there to start runs.
    s.state.set_scheduler_sink(sink());
    s.state.visions_pause(&s.ada, true).unwrap();
    s.state.visions_question_asked("ada-work", std::slice::from_ref(&question));
    let passed = passed_on(&s.state, &s.channel);
    assert_eq!(passed.len(), 1, "{passed:?}");
    assert_eq!(passed[0]["vision_id"], s.wren.as_str());
}

/// The chief's message after a turn names the workers it tasked in that turn, each once, in the
/// order it first tasked them; a turn that tasks no one names none.
#[test]
fn a_chief_message_lists_the_workers_it_tasked() {
    let s = setup(true);
    let chief = Owner {
        thread_id: "wren-chat".into(),
        vision_id: s.wren.clone(),
        channel_id: s.channel.clone(),
        kind: OwnedKind::VisionChat,
        hold: Hold::None,
    };
    for worker in [&s.ada, &"bo".to_owned(), &s.ada] {
        s.state.visions_tasked(&s.channel, &s.wren, worker);
    }
    s.state.visions_post_reply(&chief, &finished());
    s.state.visions_post_reply(&chief, &finished());
    let said: Vec<serde_json::Value> = s
        .state
        .channel_page(&s.channel, None, 1 << 20)
        .unwrap()
        .entries
        .into_iter()
        .map(|line| serde_json::to_value(line.entry).unwrap())
        .filter(|entry| entry["type"] == "vision")
        .collect();
    assert_eq!(said[0]["tasked"], serde_json::json!([s.ada, "bo"]), "{said:?}");
    assert!(said[1].get("tasked").is_none(), "{said:?}");
}

/// Ada's turn, woken for `reason`, ends (after the call `acted`, if any); who was woken then.
fn adas_turn_ends(reason: WakeReason, acted: Option<&str>) -> (Setup, Vec<String>) {
    let s = setup(true);
    let chat = Owner {
        thread_id: "ada-chat".into(),
        vision_id: s.ada.clone(),
        channel_id: s.channel.clone(),
        kind: OwnedKind::VisionChat,
        hold: Hold::None,
    };
    s.state.owner_set(chat).unwrap();
    s.state.visions_watch_silent(&s.channel, &s.ada, &[reason]);
    if let Some(method) = acted {
        s.state.visions_acted(&s.channel, &s.ada, method);
    }
    s.state.visions_after_run(&sink(), "ada-chat", &finished());
    let woken = started(&s.state);
    (s, woken)
}

/// A worker its chief tasked that ends its turn without acting (no chat started or written to,
/// no report) wakes its chief, so the task does not stall in silence.
#[test]
fn a_tasked_worker_that_does_nothing_wakes_its_chief() {
    let (s, woken) = adas_turn_ends(WakeReason::Tasked(1), None);
    assert!(woken.contains(&s.wren), "{woken:?}");
}

/// One that acted (here: reported to its chief), or one woken for something other than a task
/// (its chat finished), wakes no one as silent; a read is no act.
#[test]
fn only_a_tasked_worker_that_did_nothing_is_silent() {
    let (s, woken) = adas_turn_ends(WakeReason::Tasked(1), Some("report"));
    assert!(!woken.contains(&s.wren), "{woken:?}");
    let (s, woken) = adas_turn_ends(WakeReason::ChatFinished("w".into()), None);
    assert!(!woken.contains(&s.wren), "{woken:?}");
    let (s, woken) = adas_turn_ends(WakeReason::Tasked(1), Some("get_team_log"));
    assert!(woken.contains(&s.wren), "{woken:?}");
}

/// A team's work chat runs in its channel's mode as it stands when each run starts (Settings',
/// the project's or the channel's change applies from the chat's next run); a chat the person
/// holds, or one no Vision owns, keeps its own.
#[test]
fn a_work_chats_run_takes_its_channels_mode_when_it_starts() {
    use themis_core::configuration::ApprovalMode;
    let s = setup(true);
    own(&s.state, "ada-work", &s.ada, &s.channel, Hold::None);
    let save = |change: &dyn Fn(Channel) -> Channel| changed(&s, change);
    save(&|channel| Channel { autonomy: Some(themis_core::visions::Autonomy::Ask), ..channel });
    assert_eq!(s.state.visions_work_mode("ada-work"), Some(ApprovalMode::Custom));
    save(&|channel| Channel { full_access: true, ..channel });
    assert_eq!(s.state.visions_work_mode("ada-work"), Some(ApprovalMode::Yolo));
    own(&s.state, "ada-work", &s.ada, &s.channel, Hold::TakenOver);
    assert_eq!(s.state.visions_work_mode("ada-work"), None);
    assert_eq!(s.state.visions_work_mode("not-owned"), None);
}

/// The reviewer of a team's work chat reads what the person asked in the channel (a Vision's
/// own messages in the chat are not the person's words).
#[test]
fn the_reviewer_reads_the_persons_channel_words() {
    let s = setup(true);
    own(&s.state, "ada-work", &s.ada, &s.channel, Hold::None);
    s.state
        .channel_append(&s.channel, ChannelEntry::Person { text: "Write the omelette recipe".into() })
        .unwrap();
    let reviewer = crate::state::auto_review::DesktopReviewer {
        state: s.state.clone(),
        sink: sink(),
        thread_id: "ada-work".into(),
        run_id: "r1".into(),
        project_root: std::path::PathBuf::new(),
        config: themis_core::configuration::RuntimeConfig::default(),
        is_git: false,
        chat_model: String::new(),
    };
    assert!(reviewer.person_words().contains("Write the omelette recipe"));
}

/// When a team's work chat finishes a run, the permission questions (Auto's block boxes) it left
/// open in earlier runs are dropped: each was about one step of a run that is over, so their
/// cards go. Its ordinary questions, one from the run that just ended, and a chat no Vision
/// owns keep theirs.
#[test]
fn a_finished_chat_drops_the_questions_it_moved_on_from() {
    let s = setup(true);
    own(&s.state, "ada-work", &s.ada, &s.channel, Hold::None);
    let ask = |thread: &str, run: &str, text: &str, permission: bool| {
        let asked = NewQuestion {
            question: text.into(),
            permission,
            ..NewQuestion::default()
        };
        s.state
            .ask_question(sink().as_ref(), thread, run, asked)
            .unwrap()
            .id
    };
    ask("ada-work", "r1", "Auto mode blocked an action: touch omelette.md", true);
    let ordinary = ask("ada-work", "r1", "Which license?", false);
    let fresh = ask("ada-work", "r2", "Auto mode blocked an action: rm build", true);
    ask("plain", "r1", "Auto mode blocked an action: mine", true);
    s.state.visions_drop_stale_questions(&sink(), "ada-work", "r2");
    let open: Vec<String> = s
        .state
        .open_questions("ada-work")
        .into_iter()
        .map(|question| question.id)
        .collect();
    assert_eq!(open, [ordinary, fresh]);
    s.state.visions_drop_stale_questions(&sink(), "plain", "r2");
    assert_eq!(s.state.open_questions("plain").len(), 1);
}

/// A team chat's run never runs above its channel: a chat the person took over runs in the
/// stricter of its own mode and the channel's now, so switching Full access off always takes
/// YOLO away, and a stricter mode the person chose holds. Nothing is written back to the chat.
#[test]
fn a_held_team_chat_never_runs_above_its_channel() {
    use themis_core::configuration::ApprovalMode;
    let s = setup(true);
    own(&s.state, "ada-work", &s.ada, &s.channel, Hold::TakenOver);
    let save = |change: &dyn Fn(Channel) -> Channel| changed(&s, change);
    save(&|channel| Channel { autonomy: Some(themis_core::visions::Autonomy::Ask), ..channel });
    assert_eq!(s.state.visions_run_mode("ada-work", ApprovalMode::Yolo), ApprovalMode::Custom);
    save(&|channel| Channel { full_access: true, ..channel });
    assert_eq!(s.state.visions_run_mode("ada-work", ApprovalMode::Custom), ApprovalMode::Custom);
    assert_eq!(s.state.visions_run_mode("plain", ApprovalMode::Yolo), ApprovalMode::Yolo);
}

/// Saves `s`'s channel as `change` makes it.
fn changed(s: &Setup, change: &dyn Fn(Channel) -> Channel) {
    let channel = s.state.channel_get(&s.channel).unwrap();
    let revision = channel.revision.clone();
    s.state.channel_save(change(channel), Some(&revision)).unwrap();
}
