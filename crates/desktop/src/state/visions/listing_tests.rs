use std::sync::Arc;

use serde_json::json;
use themis_core::agent_cli::VisionScope;
use themis_core::configuration::ApprovalMode;
use themis_core::visions::Decision;

use super::*;
use crate::approvals::PendingApproval;
use crate::sink::{ChannelSink, EventSink};
use crate::state::questions::NewQuestion;
use crate::state::AppState;
use crate::types::ThreadEvent;

struct Setup {
    _dir: tempfile::TempDir,
    state: AppState,
    scope: VisionScope,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let project = project.canonicalize().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let vision = state
        .vision_save(
            Vision {
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
            },
            None,
        )
        .unwrap();
    let channel = state
        .channel_save(
            Channel {
                id: String::new(),
                name: "hq".into(),
                topic: String::new(),
                project_root: project.clone(),
                approval_mode: ApprovalMode::Custom,
                members: vec![vision.id.clone()],
                revision: String::new(),
            },
            None,
        )
        .unwrap();
    let scope = VisionScope {
        vision_id: vision.id,
        channel_id: channel.id,
        project_root: project,
        decides: Vec::new(),
    };
    Setup {
        _dir: dir,
        state,
        scope,
    }
}

fn own(s: &Setup, thread: &str, kind: OwnedKind, hold: Hold) {
    s.state
        .owner_set(Owner {
            thread_id: thread.into(),
            vision_id: s.scope.vision_id.clone(),
            channel_id: s.scope.channel_id.clone(),
            kind,
            hold,
        })
        .unwrap();
}

fn sink() -> Arc<dyn EventSink> {
    Arc::new(ChannelSink::channel().0)
}

fn ask(s: &Setup, thread: &str, text: &str) -> String {
    let asked = NewQuestion {
        question: text.into(),
        ..NewQuestion::default()
    };
    s.state
        .ask_question(sink().as_ref(), thread, "run", asked)
        .unwrap()
        .id
}

fn pass(s: &Setup, thread: &str, question: &str) {
    s.state
        .visions_pass_question(&s.scope, thread, question, Decision::Scope)
        .unwrap();
}

fn waiting_approval(s: &Setup, thread: &str, id: &str) {
    let request = serde_json::from_value(json!({"thread_id": thread, "approval_id": id,
        "tool": "shell", "summary": "rm", "risk": "execute"}))
    .unwrap();
    s.state.inner.pending.lock().unwrap().insert(
        id.into(),
        PendingApproval {
            request,
            thread_id: thread.into(),
            sender: std::sync::mpsc::channel().0,
        },
    );
}

async fn listed(s: &Setup) -> listing::VisionListed {
    s.state.visions_listed().await.remove(0)
}

fn ask_permission(s: &Setup, thread: &str, text: &str) -> String {
    let asked = NewQuestion {
        question: text.into(),
        permission: true,
        ..NewQuestion::default()
    };
    s.state
        .ask_question(sink().as_ref(), thread, "run", asked)
        .unwrap()
        .id
}

/// A permission question (an Auto block, a destructive step's box) is the person's alone, so
/// one open in a chat the Vision drives waits on the person whether or not it was passed on,
/// and counts once; in a chat the person took over it is theirs anyway.
#[tokio::test]
async fn needs_counts_open_permission_questions_once() {
    let s = setup();
    own(&s, "w1", OwnedKind::Work, Hold::None);
    own(&s, "mine", OwnedKind::Work, Hold::TakenOver);
    let block = ask_permission(&s, "w1", "Allow the push?");
    let passed_too = ask_permission(&s, "w1", "Allow the delete?");
    pass(&s, "w1", &passed_too);
    ask_permission(&s, "mine", "Allow it?");
    assert_eq!(listed(&s).await.needs, 2);
    s.state
        .record_answers("w1", vec![(block, "Allow once".into())], None)
        .unwrap();
    assert_eq!(listed(&s).await.needs, 1);
}

/// What waits on the person for a Vision is read from the chats' own state, however far back
/// the channel mentioned it: approvals waiting in its chats and the questions it passed on that
/// are still open. A chat the person took over is theirs, so what waits there is not counted.
#[tokio::test]
async fn needs_counts_approvals_and_open_passed_questions_in_chats_it_drives() {
    let s = setup();
    own(&s, "w1", OwnedKind::Work, Hold::None);
    own(&s, "mine", OwnedKind::Work, Hold::TakenOver);
    let passed = ask(&s, "w1", "Which license?");
    let answered = ask(&s, "w1", "Tabs?");
    let kept = ask(&s, "w1", "Name?");
    pass(&s, "w1", &passed);
    pass(&s, "w1", &answered);
    s.state
        .record_answers("w1", vec![(answered, "Tabs".into())], None)
        .unwrap();
    let theirs = ask(&s, "mine", "Drop Windows?");
    pass(&s, "mine", &theirs);
    waiting_approval(&s, "w1", "a1");
    waiting_approval(&s, "mine", "a2");
    waiting_approval(&s, "plain", "a3");
    assert_eq!(listed(&s).await.needs, 2, "q passed and open, a1");

    // A question the Vision still holds is its own, not the person's.
    assert!(s.state.open_questions("w1").iter().any(|q| q.id == kept));
    s.state
        .record_answers("w1", vec![(passed, "MIT".into())], None)
        .unwrap();
    s.state.inner.pending.lock().unwrap().remove("a1");
    assert_eq!(listed(&s).await.needs, 0);
}

/// Sorry follows the Vision's latest outcome: a failure in any chat it drives (its own chat
/// too) makes it sorry until a later run of its succeeds; the person's Stop is not a failure,
/// and a chat the person took over is theirs.
#[tokio::test]
async fn sorry_lasts_from_a_failure_until_its_next_success() {
    let s = setup();
    own(&s, "w1", OwnedKind::Work, Hold::None);
    own(&s, "own", OwnedKind::VisionChat, Hold::None);
    own(&s, "mine", OwnedKind::Work, Hold::TakenOver);
    let failed = ThreadEvent::Failed {
        error: "provider error".into(),
    };
    let finished = ThreadEvent::Finished {
        model: None,
        result: "done".into(),
    };
    let stopped = ThreadEvent::Failed {
        error: format!("{} now", themis_core::runtime::STOPPED_BY_YOU),
    };
    assert!(!listed(&s).await.sorry);
    s.state.visions_after_run(&sink(), "mine", &failed);
    assert!(!listed(&s).await.sorry, "the person's chat");
    s.state.visions_after_run(&sink(), "w1", &stopped);
    assert!(!listed(&s).await.sorry, "a Stop is no failure");
    s.state.visions_after_run(&sink(), "own", &failed);
    assert!(listed(&s).await.sorry, "its own chat failed");
    s.state.visions_after_run(&sink(), "w1", &finished);
    assert!(!listed(&s).await.sorry, "cleared by its next success");
    s.state.visions_after_run(&sink(), "w1", &failed);
    assert!(listed(&s).await.sorry, "a work chat failed");
}

/// A pass-on racing the chat's agent asking a new question and the person answering another
/// loses neither: all three write under the same lock. (The race is forced: the other writes
/// start while the pass-on has read the questions and not yet written them.)
#[test]
fn a_pass_on_racing_a_question_and_an_answer_loses_nothing() {
    let s = setup();
    let passed = ask(&s, "w1", "Which license?");
    let answered = ask(&s, "w1", "Tabs?");
    let racer: Arc<std::sync::Mutex<Option<std::thread::JoinHandle<String>>>> = Arc::default();
    let (state, slot, to_answer) = (s.state.clone(), Arc::clone(&racer), answered.clone());
    crate::state::questions::PASS_ON_GAP.with(|gap| {
        *gap.borrow_mut() = Some(Box::new(move || {
            let (state, to_answer) = (state.clone(), to_answer.clone());
            *slot.lock().unwrap() = Some(std::thread::spawn(move || {
                let asked = NewQuestion {
                    question: "Allow the push?".into(),
                    permission: true,
                    ..NewQuestion::default()
                };
                let sink = ChannelSink::channel().0;
                let new = state.ask_question(&sink, "w1", "run", asked).unwrap().id;
                state
                    .record_answers("w1", vec![(to_answer, "Spaces".into())], None)
                    .unwrap();
                new
            }));
            // Long enough for the other writes to land, were they not held back.
            std::thread::sleep(std::time::Duration::from_millis(200));
        }));
    });
    s.state.mark_passed_on("w1", &passed);
    crate::state::questions::PASS_ON_GAP.with(|gap| *gap.borrow_mut() = None);
    let new = racer.lock().unwrap().take().unwrap().join().unwrap();
    let questions = s.state.questions_for("w1");
    let find = |id: &str| questions.iter().find(|q| q.id == id).cloned();
    assert!(find(&passed).unwrap().passed_on, "passed on");
    assert_eq!(
        find(&answered).unwrap().answer.as_deref(),
        Some("Spaces"),
        "not reopened"
    );
    assert!(find(&new).is_some(), "the new question kept");
}

/// Records the `visions-changed` payloads that name a Vision only.
#[derive(Default)]
struct Announced(std::sync::Mutex<Vec<String>>);

impl EventSink for Announced {
    fn emit_thread_event(&self, _: &crate::types::ThreadEventEnvelope) {}
    fn emit_approval_request(&self, _: &crate::types::ApprovalRequest) {}
    fn emit_approval_expired(&self, _: &crate::types::ApprovalExpired) {}
    fn emit_review_item(&self, _: &crate::types::ReviewItem) {}
    fn emit_thread_changed(&self, _: &crate::types::ThreadChange) {}
    fn emit_settings_changed(&self, _: &crate::types::SettingsChange) {}
    fn emit_visions_changed(&self, change: &crate::types::VisionsChange) {
        if change.channel_id.is_none() {
            self.0
                .lock()
                .unwrap()
                .push(change.vision_id.clone().unwrap_or_default());
        }
    }
}

fn announcing(s: &Setup) -> Arc<Announced> {
    let announced = Arc::new(Announced::default());
    *s.state.inner.scheduler_sink.lock().unwrap() = Some(announced.clone());
    announced
}

fn taken(announced: &Announced) -> Vec<String> {
    std::mem::take(&mut *announced.0.lock().unwrap())
}

/// What waits on the person changing in a chat a Vision drives is announced at once with the
/// Vision (an Auto block or a destructive step's box asked, a question passed on, answered),
/// so "Needs you" follows; a question the Vision itself may answer, or one in a chat no Vision
/// drives, announces nothing.
#[tokio::test]
async fn questions_that_wait_on_the_person_announce_the_vision() {
    let s = setup();
    own(&s, "w1", OwnedKind::Work, Hold::None);
    let announced = announcing(&s);
    let vision = s.scope.vision_id.clone();
    let block = ask_permission(&s, "w1", "Allow the push?");
    assert_eq!(taken(&announced), vec![vision.clone()], "a block asked");
    let plain = ask(&s, "w1", "Tabs?");
    assert!(taken(&announced).is_empty(), "the Vision's own to answer");
    pass(&s, "w1", &plain);
    assert_eq!(taken(&announced), vec![vision.clone()], "passed on");
    s.state
        .record_answers("w1", vec![(block, "Allow once".into())], None)
        .unwrap();
    assert_eq!(taken(&announced), vec![vision.clone()], "answered");
    ask_permission(&s, "plain", "Allow it?");
    assert!(taken(&announced).is_empty(), "no Vision drives it");
    assert_eq!(listed(&s).await.needs, 1);
}

/// The Vision is announced when its sorry state changes (after the run's outcome is recorded),
/// not on every run end.
#[tokio::test]
async fn a_change_of_sorry_announces_the_vision() {
    let s = setup();
    own(&s, "w1", OwnedKind::Work, Hold::None);
    let announced = announcing(&s);
    let vision = s.scope.vision_id.clone();
    let failed = ThreadEvent::Failed {
        error: "provider error".into(),
    };
    let finished = ThreadEvent::Finished {
        model: None,
        result: "done".into(),
    };
    s.state.visions_mark_outcome("w1", &failed);
    assert_eq!(taken(&announced), vec![vision.clone()]);
    s.state.visions_mark_outcome("w1", &failed);
    assert!(taken(&announced).is_empty(), "still sorry");
    s.state.visions_mark_outcome("w1", &finished);
    assert_eq!(taken(&announced), vec![vision]);
    s.state.visions_mark_outcome("w1", &finished);
    assert!(taken(&announced).is_empty());
}

/// Counting what waits on the person reads each driven chat's questions once; after that a
/// listing reads none, however many chats the Visions ever drove (the writers keep the count).
#[tokio::test]
async fn listing_reads_no_questions_once_counted_however_many_chats() {
    let mut first = Vec::new();
    let mut again = Vec::new();
    for size in [20, 400] {
        let s = setup();
        for at in 0..size {
            let thread = format!("w{at}");
            own(&s, &thread, OwnedKind::Work, Hold::None);
            ask_permission(&s, &thread, "Allow it?");
        }
        // Forget the counts the writes kept, as after a restart.
        s.state.visions_cache().shown = listing::Shown::default();
        assert_eq!(listed(&s).await.needs, size);
        first.push(std::mem::take(
            &mut s.state.visions_cache().shown.chats_read,
        ));
        assert_eq!(listed(&s).await.needs, size);
        ask_permission(&s, "w0", "Allow this too?");
        assert_eq!(listed(&s).await.needs, size + 1, "kept by the writer");
        again.push(s.state.visions_cache().shown.chats_read);
    }
    assert_eq!(first, vec![20, 400], "each chat read once");
    assert_eq!(again, vec![0, 0], "none read again");
}

/// The minute re-check is started once, however often the sink is set.
#[test]
fn the_budget_recheck_loop_starts_once() {
    let s = setup();
    assert!(s.state.visions_recheck_loop_once());
    assert!(!s.state.visions_recheck_loop_once());
}
