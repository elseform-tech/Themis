//! The team grants: a chief reads and tasks its workers and drives no chat; a worker drives its
//! own work chats and reports or pushes back to its chief. Decided on the request and the owner
//! records, never on command text.

use std::collections::HashMap;

use proptest::strategy::Strategy;
use themis_core::visions::Decision;

use super::*;
use crate::server::agent::tests::grant;

fn project() -> PathBuf {
    std::env::temp_dir().canonicalize().unwrap()
}

/// `juno` leads `c1`; `rook` works there; `c2` is another channel.
fn scope(vision: &str, chief: bool) -> VisionScope {
    VisionScope {
        vision_id: vision.into(),
        channel_id: "c1".into(),
        project_root: project(),
        decides: vec![Decision::Wording, Decision::Tests],
        chief,
    }
}

fn chief() -> Grant {
    Grant {
        vision: Some(scope("juno", true)),
        ..grant()
    }
}

fn worker() -> Grant {
    Grant {
        vision: Some(scope("rook", false)),
        ..grant()
    }
}

fn owner(thread: &str, vision: &str, channel: &str, kind: OwnedKind) -> Owner {
    Owner {
        thread_id: thread.into(),
        vision_id: vision.into(),
        channel_id: channel.into(),
        kind,
        hold: Hold::None,
    }
}

/// Rook's work chat in c1, another channel's worker's chat, Rook's own Vision chat (`chat-a`,
/// where its run is), and Juno's own Vision chat.
fn owners() -> HashMap<String, Owner> {
    [
        owner("rook-work", "rook", "c1", OwnedKind::Work),
        owner("far-work", "tess", "c2", OwnedKind::Work),
        owner("chat-a", "rook", "c1", OwnedKind::VisionChat),
        owner("juno-chat", "juno", "c1", OwnedKind::VisionChat),
    ]
    .into_iter()
    .map(|owner| (owner.thread_id.clone(), owner))
    .collect()
}

fn check(grant: &Grant, method: &str, args: &Value) -> Option<Refusal> {
    let owners = owners();
    crate::server::agent::refusal(grant, method, args, &|thread| owners.get(thread).cloned())
}

fn on(thread: &str) -> Value {
    json!({"threadId": thread, "text": "go", "messageId": "m", "questionId": "q",
           "decision": "wording", "reason": "fine"})
}

#[test]
fn a_chief_reads_and_tasks_its_workers_but_drives_no_chat() {
    let task = json!({"workerId": "rook", "task": "Fix the build"});
    for method in CHIEF_CALLS {
        assert!(check(&chief(), method, &task).is_none(), "{method}");
    }
    // Reads on its worker's chat, and answering a question there within its level.
    for method in [
        "get_thread",
        "get_thread_history_page",
        "list_questions",
        "answer_question",
    ] {
        assert!(
            check(&chief(), method, &on("rook-work")).is_none(),
            "{method}"
        );
    }
    // It drives no chat: not its worker's, not a new one.
    for method in [
        "send_message",
        "enqueue_message",
        "steer_queued_message",
        "stop_thread",
    ] {
        assert!(
            check(&chief(), method, &on("rook-work")).is_some(),
            "{method}"
        );
    }
    let start = json!({"projectRoot": project()});
    assert!(check(&chief(), "create_thread", &start).is_some());
    // Above its channel's level, an answer is refused like any Vision's.
    let above =
        json!({"threadId": "rook-work", "questionId": "q", "decision": "delete", "reason": "x"});
    assert!(check(&chief(), "answer_question", &above).is_some());
    // Approvals, settings and integrations stay refused.
    for method in [
        "approve_action",
        "update_settings",
        "plugin_action",
        "set_approval_mode",
    ] {
        assert!(check(&chief(), method, &json!({})).is_some(), "{method}");
    }
    // A worker's calls are not a chief's.
    assert!(check(&chief(), "report", &json!({"text": "done"})).is_some());
}

#[test]
fn a_chief_cannot_task_another_channels_worker() {
    // Another channel's worker's chat is not readable.
    for method in ["get_thread", "list_questions", "answer_question"] {
        assert!(
            check(&chief(), method, &on("far-work")).is_some(),
            "{method}"
        );
    }
    // A chief call naming another channel is refused on its face.
    let elsewhere = json!({"workerId": "tess", "task": "x", "channelId": "c2"});
    assert!(check(&chief(), "assign_task", &elsewhere).is_some());
}

#[test]
fn a_worker_reports_and_pushes_back_only_to_its_chief() {
    assert!(check(&worker(), "report", &json!({"text": "Done: build green"})).is_none());
    let back = json!({"text": "This deletes data", "safer": "Archive it instead"});
    assert!(check(&worker(), "push_back", &back).is_none());
    // Its own team log only.
    assert!(check(&worker(), "get_team_log", &json!({})).is_none());
    assert!(check(&worker(), "get_team_log", &json!({"workerId": "rook"})).is_none());
    assert!(check(&worker(), "get_team_log", &json!({"workerId": "wren"})).is_some());
    // Phase 1's own-chat driving still holds.
    assert!(check(&worker(), "send_message", &on("rook-work")).is_none());
}

#[test]
fn a_worker_cannot_post_in_the_channel_or_task_others() {
    for method in CHIEF_CALLS
        .iter()
        .filter(|method| **method != "get_team_log")
    {
        let args = json!({"workerId": "wren", "task": "x", "text": "x"});
        assert!(check(&worker(), method, &args).is_some(), "{method}");
    }
    for method in ["post_channel_message", "send_message", "get_thread"] {
        assert!(
            check(&worker(), method, &on("juno-chat")).is_some(),
            "{method}"
        );
    }
    assert!(check(&worker(), "send_message", &on("far-work")).is_some());
}

/// Every method a team call might name, plus noise.
const KNOWN: &[&str] = &[
    "assign_task",
    "redirect_worker",
    "pause_worker",
    "resume_worker",
    "get_team_log",
    "list_team",
    "report",
    "push_back",
    "send_message",
    "enqueue_message",
    "steer_queued_message",
    "stop_thread",
    "answer_question",
    "pass_question",
    "list_questions",
    "create_thread",
    "get_thread",
    "get_thread_history_page",
    "approve_action",
    "update_settings",
    "post_channel_message",
    "plugin_action",
];

proptest::proptest! {
    /// Whatever a chief or worker names, a call that passes stays inside its team: a chat it
    /// touches is its own or (for the chief) its own channel's worker's, and nothing drives a
    /// chat for a chief.
    #[test]
    fn arbitrary_team_calls_stay_inside_the_team(
        method in proptest::prop_oneof![
            3 => proptest::sample::select(KNOWN).prop_map(str::to_owned),
            1 => proptest::string::string_regex("[a-z_]{0,24}").unwrap(),
        ],
        is_chief in proptest::bool::ANY,
        thread in proptest::option::of(proptest::sample::select(
            &["rook-work", "far-work", "chat-a", "juno-chat", "plain"][..])),
        worker_id in proptest::option::of(proptest::sample::select(&["rook", "tess", "juno"][..])),
    ) {
        let grant = if is_chief { chief() } else { worker() };
        let mut args = json!({"text": "go", "task": "go", "questionId": "q",
                              "decision": "wording", "reason": "ok"});
        if let Some(thread) = thread { args["threadId"] = json!(thread); }
        if let Some(worker) = worker_id { args["workerId"] = json!(worker); }
        if check(&grant, &method, &args).is_none() {
            let own = if is_chief { "juno" } else { "rook" };
            // Team calls act on team logs by `workerId`, never on a chat named in the call.
            let team_call = CHIEF_CALLS.contains(&method.as_str())
                || WORKER_CALLS.contains(&method.as_str());
            if let (Some(thread), false) = (thread, team_call) {
                let theirs = owners().get(thread).cloned();
                let inside = match &theirs {
                    // Its run's own chat is chat-a for both test grants.
                    _ if thread == "chat-a" => true,
                    Some(owner) => owner.channel_id == "c1"
                        && (owner.vision_id == own || (is_chief && owner.kind == OwnedKind::Work)),
                    None => false,
                };
                proptest::prop_assert!(inside, "{} on {}", method, thread);
            }
            if is_chief {
                let drives = ["send_message", "enqueue_message", "steer_queued_message",
                              "stop_thread", "create_thread"];
                proptest::prop_assert!(!drives.contains(&method.as_str()), "{}", method);
                proptest::prop_assert!(!["report", "push_back"].contains(&method.as_str()));
            } else {
                let tasks = ["assign_task", "redirect_worker", "pause_worker", "resume_worker", "list_team"];
                proptest::prop_assert!(!tasks.contains(&method.as_str()), "{}", method);
                if method == "get_team_log" {
                    proptest::prop_assert!(worker_id.is_none_or(|w| w == "rook"));
                }
            }
        }
    }
}
