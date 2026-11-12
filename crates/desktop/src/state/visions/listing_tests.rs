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
