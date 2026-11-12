use std::collections::{BTreeMap, HashMap};

use proptest::strategy::Strategy;
use themis_core::configuration::VisionsConfig;
use themis_core::visions::{Autonomy, Decision};

use super::*;
use crate::server::agent::tests::{grant, REFUSED};
use crate::state::{vision_decisions, Vision};

fn project() -> PathBuf {
    std::env::temp_dir().canonicalize().unwrap()
}

fn scope() -> VisionScope {
    VisionScope {
        vision_id: "v1".into(),
        channel_id: "c1".into(),
        project_root: project(),
        // Small calls, the default level.
        decides: vec![Decision::Wording, Decision::Tests],
    }
}

/// A Vision's grant: its run is in its own Vision chat, `chat-a`.
fn vision_grant() -> Grant {
    Grant {
        vision: Some(scope()),
        ..grant()
    }
}

fn owner(thread: &str, vision: &str, kind: OwnedKind, hold: Hold) -> Owner {
    Owner {
        thread_id: thread.into(),
        vision_id: vision.into(),
        channel_id: "c1".into(),
        kind,
        hold,
    }
}

/// `work` is the Vision's work chat (held as given), `theirs` another Vision's, `chat-a` its
/// own Vision chat; `plain` has no owner.
fn owners(hold: Hold) -> HashMap<String, Owner> {
    [
        owner("work", "v1", OwnedKind::Work, hold),
        owner("theirs", "v2", OwnedKind::Work, Hold::None),
        owner("chat-a", "v1", OwnedKind::VisionChat, Hold::None),
    ]
    .into_iter()
    .map(|owner| (owner.thread_id.clone(), owner))
    .collect()
}

fn check(
    grant: &Grant,
    method: &str,
    args: &Value,
    owners: &HashMap<String, Owner>,
) -> Option<Refusal> {
    super::super::refusal(grant, method, args, &|thread| owners.get(thread).cloned())
}

fn reason(refusal: Option<Refusal>) -> Option<Reason> {
    refusal.map(|refused| refused.reason)
}

fn on(thread: &str) -> Value {
    json!({"threadId": thread, "text": "go", "messageId": "m", "questionId": "q"})
}

/// The driving methods that name a chat (`threadId`).
fn chat_methods() -> impl Iterator<Item = &'static str> {
    VISION_DRIVE
        .iter()
        .copied()
        .filter(|method| !matches!(*method, "create_thread" | "list_channel_threads"))
}

#[test]
fn a_vision_grant_reaches_only_its_own_work_chats() {
    let grant = vision_grant();
    let owners = owners(Hold::None);
    for method in chat_methods() {
        assert_eq!(
            check(&grant, method, &on("work"), &owners),
            None,
            "{method}"
        );
        for elsewhere in [on("theirs"), on("plain"), on("chat-a"), json!({})] {
            assert_eq!(
                reason(check(&grant, method, &elsewhere, &owners)),
                Some(Reason::MethodNotAllowed),
                "{method} {elsewhere}"
            );
        }
        // An ordinary run's credential reaches none of them, owned or not.
        assert!(check(&grant_without_vision(), method, &on("work"), &owners).is_some());
    }
    for method in OWN_CHAT {
        assert_eq!(
            check(&grant, method, &on("work"), &owners),
            None,
            "{method}"
        );
        assert_eq!(
            check(&grant, method, &on("chat-a"), &owners),
            None,
            "{method}"
        );
        for elsewhere in [on("theirs"), on("plain")] {
            assert!(
                check(&grant, method, &elsewhere, &owners).is_some(),
                "{method}"
            );
        }
    }
    // The run's own chat reads stay its own chat's.
    for method in CHAT_READS {
        assert!(check(&grant, method, &on("work"), &owners).is_some());
    }
    let channel = |id: Value| json!({"channelId": id});
    let list = "list_channel_threads";
    assert_eq!(check(&grant, list, &channel(json!("c1")), &owners), None);
    for other in [json!("c2"), Value::Null, json!(1)] {
        assert_eq!(
            reason(check(&grant, list, &channel(other), &owners)),
            Some(Reason::MethodNotAllowed)
        );
    }
}

fn grant_without_vision() -> Grant {
    grant()
}

fn answer(decision: &str) -> Value {
    json!({"threadId": "work", "questionId": "q", "answer": "MIT", "decision": decision,
        "reason": "the usual choice"})
}

fn profile(autonomy: Autonomy, custom: Option<BTreeMap<String, bool>>) -> Vision {
    Vision {
        id: "v1".into(),
        name: "Wren".into(),
        role: String::new(),
        shape: "cloud".into(),
        color: "teal".into(),
        model: None,
        autonomy: Some(autonomy),
        custom,
        daily_budget_usd: None,
        soul: String::new(),
        paused: false,
        revision: String::new(),
    }
}

/// A Vision answers a question itself only on a decision its autonomy leaves to it; above that
/// its answer is refused (the question stays the person's), but it may always pass one on. Sending
/// work out and spending are never its own, Custom with every row on included.
#[test]
fn a_vision_answers_only_within_its_autonomy() {
    let owners = owners(Hold::None);
    let small = vision_grant();
    assert_eq!(
        check(&small, "answer_question", &answer("wording"), &owners),
        None
    );
    for above in ["undoable", "scope", "delete", "external", "spend"] {
        let refused = check(&small, "answer_question", &answer(above), &owners).expect(above);
        assert_eq!(refused.reason, Reason::AboveAutonomy);
        assert_eq!(refused.reason.label(), "above_autonomy");
        assert_eq!(
            refused.message,
            "Your autonomy level leaves this to the person; pass it on in the channel."
        );
        let pass = json!({"threadId": "work", "questionId": "q", "decision": above});
        assert_eq!(
            check(&small, "pass_question", &pass, &owners),
            None,
            "{above}"
        );
    }
    // Every row on, as a hand-edited file might say: still never `external` or `spend`.
    let rows: BTreeMap<String, bool> = Decision::ALL
        .iter()
        .map(|decision| (decision.id().to_owned(), true))
        .collect();
    let config = VisionsConfig {
        autonomy: Autonomy::Custom,
        custom: rows.clone(),
        ..VisionsConfig::default()
    };
    let decides = vision_decisions(&profile(Autonomy::Custom, Some(rows)), &config);
    let custom = Grant {
        vision: Some(VisionScope { decides, ..scope() }),
        ..grant()
    };
    for decision in Decision::ALL {
        let refused = check(&custom, "answer_question", &answer(decision.id()), &owners);
        let theirs = matches!(decision, Decision::External | Decision::Spend);
        assert_eq!(
            reason(refused),
            theirs.then_some(Reason::AboveAutonomy),
            "{}",
            decision.id()
        );
    }
    // Another Vision's chat: refused for ownership first, whatever the decision.
    let theirs = json!({"threadId": "theirs", "questionId": "q", "decision": "wording"});
    for method in ["answer_question", "pass_question"] {
        assert_eq!(
            reason(check(&small, method, &theirs, &owners)),
            Some(Reason::MethodNotAllowed)
        );
    }
}

/// Stopped by the person: the Vision may not write there, but may still read and stop it. Taken
/// over: the run is the person's, so it may not stop it either.
#[test]
fn a_held_chat_refuses_the_visions_writes() {
    let grant = vision_grant();
    for (hold, blocked) in [
        (Hold::StoppedByPerson, WRITES.to_vec()),
        (Hold::TakenOver, [WRITES, &["stop_thread"]].concat()),
    ] {
        let owners = owners(hold);
        for method in chat_methods().chain(OWN_CHAT.iter().copied()) {
            let refused = check(&grant, method, &on("work"), &owners);
            if blocked.contains(&method) {
                let refused = refused.expect(method);
                assert_eq!(refused.reason, Reason::HeldByPerson);
                assert_eq!(refused.reason.label(), "held_by_person");
                assert_eq!(
                    refused.message,
                    "The person stopped or took over this chat; wait for them to hand it back."
                );
            } else {
                assert_eq!(refused, None, "{hold:?} {method}");
            }
        }
    }
}

/// The person's methods stay refused to a Vision, on its own chats too: it never approves,
/// changes a mode, writes settings or keys, or reaches the Visions methods.
#[test]
fn a_vision_grant_never_approves_or_changes_modes() {
    let grant = vision_grant();
    let owners = owners(Hold::None);
    for method in REFUSED {
        for args in [on("plain"), on("theirs"), on("chat-a"), Value::Null] {
            assert_eq!(
                reason(check(&grant, method, &args, &owners)),
                Some(Reason::MethodNotAllowed),
                "{method} {args}"
            );
        }
        if !VISION_DRIVE.contains(method) {
            let args = json!({"threadId": "work", "mode": "yolo", "decision": "allow"});
            assert_eq!(
                reason(check(&grant, method, &args, &owners)),
                Some(Reason::MethodNotAllowed),
                "{method}"
            );
        }
    }
    // Spelled out: approving and changing the mode stay refused on its own work chat.
    for (method, args) in [
        (
            "approve_action",
            json!({"threadId": "work", "approvalId": "a", "decision": "once"}),
        ),
        (
            "set_thread_approval_mode",
            json!({"threadId": "work", "mode": "yolo"}),
        ),
    ] {
        assert_eq!(
            reason(check(&grant, method, &args, &owners)),
            Some(Reason::MethodNotAllowed),
            "{method}"
        );
    }
    let channel = json!({"threadId": "work", "channelId": "c1", "visionId": "v1"});
    for method in crate::server::visions_commands::METHODS {
        if *method != "list_channel_threads" {
            assert_eq!(
                reason(check(&grant, method, &channel, &owners)),
                Some(Reason::MethodNotAllowed),
                "{method}"
            );
        }
    }
    // Switching a hook or a command-based server on stays the store's to refuse: the guard
    // lets only listed `plugin_action` actions through, as for every run.
    let runs = json!({"action": "test_hook"});
    assert_eq!(
        reason(check(&grant, "plugin_action", &runs, &owners)),
        Some(Reason::RunsCode)
    );
}

#[test]
fn create_thread_is_refused_outside_the_channels_project() {
    let project = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let grant = Grant {
        vision: Some(VisionScope {
            project_root: root.clone(),
            ..scope()
        }),
        ..grant()
    };
    let owners = HashMap::new();
    let create = |root: Value| json!({"projectRoot": root, "provider": "go"});
    assert_eq!(
        check(&grant, "create_thread", &create(json!(root)), &owners),
        None
    );
    let spelled = format!(" {}/./ ", project.path().display());
    assert_eq!(
        check(&grant, "create_thread", &create(json!(spelled)), &owners),
        None
    );
    for elsewhere in [
        json!(other.path()),
        json!(root.join("missing")),
        json!(format!("{}/..", root.display())),
        json!(""),
        Value::Null,
        json!(7),
    ] {
        let refused =
            check(&grant, "create_thread", &create(elsewhere.clone()), &owners).expect("refused");
        assert_eq!(refused.reason, Reason::OtherProject, "{elsewhere}");
        assert_eq!(
            refused.message,
            themis_core::prompts::AGENT_CLI_VISION_OTHER_PROJECT
        );
    }
    // An ordinary run's credential never starts a chat.
    assert_eq!(
        reason(check(
            &grant_without_vision(),
            "create_thread",
            &create(json!(root)),
            &owners
        )),
        Some(Reason::MethodNotAllowed)
    );
}

/// The shell treats every driving method as an execution, never a read: each one changes a chat
/// or reads one the run does not own, so the approval step sees it.
#[test]
fn the_shell_never_reads_a_driving_method() {
    use themis_core::tools::themis_cli::{classify, ThemisCall};
    for method in VISION_DRIVE {
        let command: Vec<String> = ["themis", "call", method, "{}"]
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(classify(&command), Some(ThemisCall::Execute), "{method}");
    }
}

/// What the rules promise, written out independently of them.
fn expected(
    method: &str,
    thread: Option<&str>,
    channel: Option<&str>,
    in_project: Option<bool>,
    owners: &HashMap<String, Owner>,
) -> bool {
    let owned = thread
        .and_then(|thread| owners.get(thread))
        .filter(|owner| owner.vision_id == "v1" && owner.kind == OwnedKind::Work);
    let writes = matches!(
        method,
        "send_message" | "enqueue_message" | "steer_queued_message" | "answer_question"
    );
    let held = owned.is_some_and(|owner| match owner.hold {
        Hold::None => false,
        Hold::StoppedByPerson => writes,
        Hold::TakenOver => writes || method == "stop_thread",
    });
    match method {
        "create_thread" => in_project == Some(true),
        "list_channel_threads" => channel == Some("c1"),
        _ if VISION_DRIVE.contains(&method) => owned.is_some() && !held,
        _ if OWN_CHAT.contains(&method) => owned.is_some() || thread == Some("chat-a"),
        // The grant names no project, so a `plugin_action` naming one is refused.
        "plugin_action" => in_project.is_none() && thread.is_none_or(|thread| thread == "chat-a"),
        _ if ALLOWED.contains(&method) => thread.is_none_or(|thread| thread == "chat-a"),
        _ => false,
    }
}

fn hold_of(index: u8) -> Hold {
    match index {
        0 => Hold::None,
        1 => Hold::StoppedByPerson,
        _ => Hold::TakenOver,
    }
}

/// Methods the generated calls draw from most of the time, beside arbitrary names.
const KNOWN: &[&str] = &[
    "create_thread",
    "send_message",
    "enqueue_message",
    "steer_queued_message",
    "stop_thread",
    "answer_question",
    "pass_question",
    "list_questions",
    "list_channel_threads",
    "get_thread",
    "get_thread_tasks",
    "approve_action",
    "set_thread_approval_mode",
    "plugin_action",
    "ping",
    "recall",
];

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(4096))]

    /// Whatever a Vision's run sends, with whatever chats owned by whom, a call passes only for
    /// a listed method, on a work chat its Vision owns (or its own chat for reads), and never
    /// a write while the person holds that chat.
    #[test]
    fn arbitrary_vision_calls_pass_only_for_owned_chats(
        method in proptest::prop_oneof![
            3 => proptest::sample::select(KNOWN).prop_map(str::to_owned),
            1 => proptest::string::string_regex("[a-z_]{0,24}|\\PC{0,12}").unwrap(),
        ],
        thread in proptest::option::of(proptest::prop_oneof![
            4 => proptest::string::string_regex("t[0-4]").unwrap(),
            1 => proptest::strategy::Just("chat-a".to_owned()),
            1 => proptest::string::string_regex("[a-z-]{0,6}").unwrap(),
        ]),
        channel in proptest::option::of("c[1-2]"),
        in_project in proptest::option::of(proptest::bool::ANY),
        // Whose (none, this Vision twice as likely, another), work chat (2 in 3), hold.
        table in proptest::collection::vec((0u8..4, 0u8..3, 0u8..3), 5),
    ) {
        let mut owners: HashMap<String, Owner> = HashMap::new();
        for (index, (whose, work, hold)) in table.iter().enumerate() {
            let vision = match whose { 0 => continue, 1 | 2 => "v1", _ => "v2" };
            let kind = if *work < 2 { OwnedKind::Work } else { OwnedKind::VisionChat };
            let thread = format!("t{index}");
            owners.insert(thread.clone(), owner(&thread, vision, kind, hold_of(*hold)));
        }
        owners.insert("chat-a".into(), owner("chat-a", "v1", OwnedKind::VisionChat, Hold::None));
        let mut args = json!({"action": "list", "text": "go"});
        if let Some(thread) = &thread { args["threadId"] = json!(thread); }
        if let Some(channel) = &channel { args["channelId"] = json!(channel); }
        if let Some(inside) = in_project {
            let root = if inside { project() } else { project().join("no-such-dir") };
            args["projectRoot"] = json!(root);
        }
        let passed = check(&vision_grant(), &method, &args, &owners).is_none();
        let want = expected(&method, thread.as_deref(), channel.as_deref(), in_project, &owners);
        proptest::prop_assert_eq!(passed, want, "{} {}", method, args);
    }
}
