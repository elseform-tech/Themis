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
    s.state.set_scheduler_sink(sink());
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
