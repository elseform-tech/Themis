//! `themis` calls in an agent's shell at the approval step, in each mode.
use super::*;
use crate::tools::{AutoReviewer, AutoVerdict};
use std::sync::atomic::{AtomicUsize, Ordering};

fn call(argv: &[&str]) -> ToolAction {
    let command: Vec<String> = argv.iter().map(|word| (*word).to_owned()).collect();
    ToolAction {
        display: None,
        tool: "shell".into(),
        summary: format!("run `{}`", argv.join(" ")),
        risk: crate::tools::shell_risk(&command),
        command,
    }
}

fn policy(rules: Option<&str>) -> ApprovalPolicy {
    resolve(rules, None, None).unwrap().config.approval
}

/// The person: counts how often they are asked and gives `answer`.
fn person(asked: &Arc<AtomicUsize>, answer: Approval) -> Arc<dyn ApprovalHook> {
    let asked = asked.clone();
    Arc::new(crate::tools::FnHook::new(move |_| {
        asked.fetch_add(1, Ordering::SeqCst);
        answer
    }))
}

fn count(n: &AtomicUsize) -> usize {
    n.load(Ordering::SeqCst)
}

const LIST: &[&str] = &["themis", "plugin", "list"];
const CREATE: &[&str] = &["themis", "plugin", "create", "demo"];
const HOOK_ON: &[&str] = &["themis", "hook", "enable", "demo", "guard"];
const UNINSTALL: &[&str] = &["themis", "plugin", "uninstall", "demo"];

/// Custom: reads pass without a prompt; a change asks once and "Allow in this chat" covers the
/// same command; switching on a hook and removing ask every time, a chat grant never covers
/// them, and wrapping them changes nothing.
#[test]
fn custom_reads_pass_changes_ask_and_switching_on_asks_every_time() {
    let asked = Arc::new(AtomicUsize::new(0));
    let grants = ChatGrants::in_memory();
    let hook = configured_hook_with_grants(
        ApprovalMode::Custom,
        policy(None),
        person(&asked, Approval::AllowAlways),
        grants.clone(),
    );
    assert_eq!(hook.approve(&call(LIST)), Approval::AllowOnce);
    assert!(!hook.would_ask(&call(LIST)));
    assert_eq!(count(&asked), 0, "a read asks nobody");

    assert_eq!(hook.approve(&call(CREATE)), Approval::AllowOnce);
    assert_eq!(count(&asked), 1);
    assert!(grants.covers(&call(CREATE)), "allowed in this chat");
    assert_eq!(hook.approve(&call(CREATE)), Approval::AllowOnce);
    assert_eq!(count(&asked), 1, "the chat grant covers the same command");
    assert!(
        !grants.covers(&call(&["themis", "plugin", "create", "other"])),
        "a grant covers the same command, not every `themis` call"
    );

    for argv in [
        HOOK_ON,
        &["themis", "mcp", "enable", "demo", "docs"],
        &["themis", "plugin", "enable", "demo"],
        UNINSTALL,
        &[
            "sh",
            "-c",
            "themis plugin list && themis hook enable demo guard",
        ],
        &["env", "A=1", "themis", "hook", "enable", "demo", "guard"],
    ] {
        let before = count(&asked);
        assert_eq!(hook.approve(&call(argv)), Approval::AllowOnce, "{argv:?}");
        assert_eq!(hook.approve(&call(argv)), Approval::AllowOnce, "{argv:?}");
        assert_eq!(count(&asked), before + 2, "asked every time: {argv:?}");
        assert!(!grants.covers(&call(argv)), "{argv:?}");
    }
    // A grant written by hand for switching on a hook is refused when loaded.
    let forged = crate::chat_grants::GrantEntry {
        tool: "shell".into(),
        command: HOOK_ON.iter().map(|word| (*word).to_owned()).collect(),
        server: None,
        risk: "execute".into(),
    };
    assert!(forged.validate().is_err());
}

/// A broad allow rule trusts `shell`, not switching on a hook; a rule naming the call does.
#[test]
fn only_a_rule_naming_it_allows_switching_on_a_hook() {
    let broad = policy(Some(
        r#"{"approval":{"rules":[{"tool":"shell","action":"allow"}]}}"#,
    ));
    assert_eq!(broad.decision(&call(CREATE)), PolicyAction::Allow);
    assert_eq!(broad.decision(&call(HOOK_ON)), PolicyAction::Ask);
    let by_risk = policy(Some(
        r#"{"approval":{"rules":[{"tool":"*","risk":"execute","action":"allow"}]}}"#,
    ));
    assert_eq!(by_risk.decision(&call(HOOK_ON)), PolicyAction::Ask);
    let prefix = policy(Some(
        r#"{"approval":{"rules":[{"tool":"shell","command":["themis"],"action":"allow"}]}}"#,
    ));
    assert_eq!(prefix.decision(&call(CREATE)), PolicyAction::Allow);
    assert_eq!(prefix.decision(&call(HOOK_ON)), PolicyAction::Ask);
    let named = policy(Some(
        r#"{"approval":{"rules":[{"tool":"shell","command":["themis","hook","enable"],"action":"allow"}]}}"#,
    ));
    assert_eq!(named.decision(&call(HOOK_ON)), PolicyAction::Allow);
}

struct Reviewer {
    reviewed: AtomicUsize,
    told: AtomicUsize,
    allowed_once: bool,
}

impl AutoReviewer for Reviewer {
    fn review(&self, _: &ToolAction) -> AutoVerdict {
        self.reviewed.fetch_add(1, Ordering::SeqCst);
        AutoVerdict::Allow
    }
    fn blocked(&self, _: &ToolAction, _: &str, _: &str) {
        self.told.fetch_add(1, Ordering::SeqCst);
    }
    fn allowed_once(&self, _: &ToolAction) -> bool {
        self.allowed_once
    }
}

fn auto(allowed_once: bool) -> (Arc<dyn ApprovalHook>, Arc<Reviewer>, Arc<AtomicUsize>) {
    let reviewer = Arc::new(Reviewer {
        reviewed: AtomicUsize::new(0),
        told: AtomicUsize::new(0),
        allowed_once,
    });
    let asked = Arc::new(AtomicUsize::new(0));
    let hook = configured_hook_with_reviewer(
        ApprovalMode::Auto,
        policy(None),
        person(&asked, Approval::AllowOnce),
        ChatGrants::in_memory(),
        None,
        Some(reviewer.clone() as Arc<dyn AutoReviewer>),
    );
    (hook, reviewer, asked)
}

/// Auto: reads need nobody, changes, tests and removals are the reviewer's, and switching on
/// a hook goes to the person even when the reviewer would allow everything.
#[test]
fn auto_sends_switching_on_a_hook_to_the_person_not_the_reviewer() {
    let (hook, reviewer, dialog) = auto(false);
    assert_eq!(hook.approve(&call(LIST)), Approval::AllowOnce);
    assert_eq!(count(&reviewer.reviewed), 0, "a read needs no review");
    for argv in [
        CREATE,
        UNINSTALL,
        &["themis", "hook", "test", "demo", "guard"],
    ] {
        assert_eq!(hook.approve(&call(argv)), Approval::AllowOnce, "{argv:?}");
    }
    assert_eq!(count(&reviewer.reviewed), 3, "the reviewer decides these");

    for argv in [
        HOOK_ON,
        &["themis", "mcp", "enable", "demo", "docs"],
        &["sh", "-c", "themis hook enable demo guard"],
        &["themis", "hook", "not-a-command"],
    ] {
        let action = call(argv);
        assert_eq!(hook.approve(&action), Approval::DenyReviewed, "{argv:?}");
        let note = crate::tools::take_review_note(&action).unwrap();
        assert!(
            note.contains(crate::prompts::AUTO_SWITCH_ON_REASON),
            "{note}"
        );
        assert!(
            note.contains(crate::prompts::AUTO_SWITCH_ON_SAFER),
            "{note}"
        );
    }
    assert_eq!(count(&reviewer.reviewed), 3, "the model never judges these");
    assert_eq!(count(&reviewer.told), 4, "the person is asked each time");
    assert_eq!(count(&dialog), 0, "through the question box, not a dialog");

    // The person's own "Allow once" after the block lets it run.
    let (hook, reviewer, _) = auto(true);
    assert_eq!(hook.approve(&call(HOOK_ON)), Approval::AllowOnce);
    assert_eq!(count(&reviewer.reviewed), 0);
}

/// The off switch: a deny rule for shell commands starting with `themis` blocks every `themis`
/// call in every mode, YOLO included, bare, by path or wrapped.
#[test]
fn a_deny_rule_turns_themis_off_in_every_mode() {
    let rules = r#"{"approval":{"rules":[{"tool":"shell","command":["themis"],"action":"deny"}]}}"#;
    for mode in [ApprovalMode::Yolo, ApprovalMode::Auto, ApprovalMode::Custom] {
        let asked = Arc::new(AtomicUsize::new(0));
        let hook = configured_hook(
            mode,
            policy(Some(rules)),
            person(&asked, Approval::AllowOnce),
        );
        for argv in [
            LIST,
            HOOK_ON,
            &["themis", "status"],
            &["/opt/bin/themis", "plugin", "list"],
            &["sh", "-c", "echo hi && themis plugin list"],
            &["env", "A=1", "themis", "status"],
        ] {
            assert_eq!(
                hook.approve(&call(argv)),
                Approval::Deny,
                "{mode:?} {argv:?}"
            );
            assert!(hook.denied_by_rule(&call(argv)), "{mode:?} {argv:?}");
        }
        assert_eq!(count(&asked), 0, "{mode:?}");
        if mode == ApprovalMode::Yolo {
            assert_eq!(hook.approve(&call(&["echo", "hi"])), Approval::AllowOnce);
        }
    }
}
