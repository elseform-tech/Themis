//! The shapes the app reads (`web/src/lib/visions.ts`) against one fixture both sides test:
//! each value here serializes to exactly the fixture's and reads back the same. Every field is
//! named and every variant matched, so a new one fails to compile here until the fixture has it,
//! and then the app's fixture test fails until its types do.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde_json::Value;
use themis_core::configuration::ApprovalMode;
use themis_core::visions::Autonomy;

use super::listing::{VisionChat, VisionListed};
use super::*;

const FIXTURE: &str = include_str!("../../../../../web/src/lib/visions.fixture.json");

fn fixture(key: &str) -> Value {
    let all: Value = serde_json::from_str(FIXTURE).unwrap();
    all[key].clone()
}

/// `value` is the fixture's `key` and reads back unchanged.
fn same<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(value: &T, key: &str) {
    let wire = serde_json::to_value(value).unwrap();
    assert_eq!(wire, fixture(key), "{key} drifted from the app's fixture");
    assert_eq!(&serde_json::from_value::<T>(wire).unwrap(), value);
}

fn line(seq: u64, entry: ChannelEntry) -> ChannelLine {
    ChannelLine {
        seq,
        at_ms: seq * 1000,
        entry,
    }
}

/// Every entry, in the fixture's order; the match keeps it to one per variant.
fn every_entry() -> Vec<ChannelEntry> {
    let entries = vec![
        ChannelEntry::Person {
            text: "@wren ship it".into(),
        },
        ChannelEntry::Vision {
            vision_id: "v1".into(),
            text: "On it.".into(),
        },
        ChannelEntry::ChatStarted {
            vision_id: "v1".into(),
            thread_id: "w1".into(),
            title: "Release".into(),
        },
        ChannelEntry::Answered {
            vision_id: "v1".into(),
            thread_id: "w1".into(),
            question_id: "q1".into(),
            decision: "tests".into(),
            question: "Add a test?".into(),
            answer: "Yes".into(),
            reason: "Cheap to undo".into(),
        },
        ChannelEntry::PassedOn {
            vision_id: "v1".into(),
            thread_id: "w1".into(),
            question_id: "q2".into(),
            decision: "scope".into(),
            question: "Drop Windows?".into(),
        },
        ChannelEntry::Notice {
            kind: NoticeKind::BudgetUnknown,
            vision_id: Some("v1".into()),
            thread_id: Some("w1".into()),
            unpriced_requests: Some(3),
            reason: Some("usage_unreadable".into()),
        },
    ];
    for (at, entry) in entries.iter().enumerate() {
        let place = match entry {
            ChannelEntry::Person { .. } => 0,
            ChannelEntry::Vision { .. } => 1,
            ChannelEntry::ChatStarted { .. } => 2,
            ChannelEntry::Answered { .. } => 3,
            ChannelEntry::PassedOn { .. } => 4,
            ChannelEntry::Notice { .. } => 5,
        };
        assert_eq!(place, at);
    }
    entries
}

/// Every notice kind, in the fixture's order.
fn every_notice_kind() -> Vec<NoticeKind> {
    let kinds = vec![
        NoticeKind::Stopped,
        NoticeKind::Paused,
        NoticeKind::Resumed,
        NoticeKind::BudgetReached,
        NoticeKind::ChatFailed,
        NoticeKind::HandedBack,
        NoticeKind::SandboxUnavailable,
        NoticeKind::BudgetUnknown,
        NoticeKind::Unpriced,
    ];
    for (at, kind) in kinds.iter().enumerate() {
        let place = match kind {
            NoticeKind::Stopped => 0,
            NoticeKind::Paused => 1,
            NoticeKind::Resumed => 2,
            NoticeKind::BudgetReached => 3,
            NoticeKind::ChatFailed => 4,
            NoticeKind::HandedBack => 5,
            NoticeKind::SandboxUnavailable => 6,
            NoticeKind::BudgetUnknown => 7,
            NoticeKind::Unpriced => 8,
        };
        assert_eq!(place, at);
    }
    kinds
}

#[test]
fn channel_lines_match_the_apps_fixture() {
    let lines: Vec<ChannelLine> = every_entry()
        .into_iter()
        .zip(1..)
        .map(|(entry, seq)| line(seq, entry))
        .collect();
    same(&lines, "lines");
    same(&every_notice_kind(), "notice_kinds");
}

#[test]
fn listed_visions_channels_and_owners_match_the_apps_fixture() {
    let listed = VisionListed {
        vision: Vision {
            id: "v1".into(),
            name: "Wren".into(),
            role: "Release captain".into(),
            shape: "cloud".into(),
            color: "teal".into(),
            model: Some("go/test-model".into()),
            autonomy: Some(Autonomy::Custom),
            custom: Some(BTreeMap::from([
                ("tests".into(), true),
                ("wording".into(), false),
            ])),
            daily_budget_usd: Some(0.5),
            soul: "Careful.".into(),
            paused: false,
            revision: "r1".into(),
        },
        over_budget: true,
        vision_chats: vec![VisionChat {
            thread_id: "own1".into(),
            channel_id: "c1".into(),
            running: true,
        }],
        needs: 2,
        sorry: true,
    };
    same(&listed, "vision");
    let channel = listing::ChannelListed {
        channel: Channel {
            id: "c1".into(),
            name: "hq".into(),
            topic: "Ship 2.0".into(),
            project_root: "/repo".into(),
            approval_mode: ApprovalMode::Auto,
            members: vec!["v1".into()],
            revision: "r2".into(),
        },
        needs: 1,
    };
    same(&channel, "channel");
    let owner = Owner {
        thread_id: "w1".into(),
        vision_id: "v1".into(),
        channel_id: "c1".into(),
        kind: OwnedKind::Work,
        hold: Hold::TakenOver,
    };
    same(&owner, "owner");
}

/// The value unions the app names, every variant in the fixture's order.
#[test]
fn holds_kinds_and_modes_match_the_apps_fixture() {
    let holds = [Hold::None, Hold::StoppedByPerson, Hold::TakenOver];
    for (at, hold) in holds.iter().enumerate() {
        let place = match hold {
            Hold::None => 0,
            Hold::StoppedByPerson => 1,
            Hold::TakenOver => 2,
        };
        assert_eq!(place, at);
    }
    same(&holds.to_vec(), "holds");
    let kinds = [OwnedKind::VisionChat, OwnedKind::Work];
    for (at, kind) in kinds.iter().enumerate() {
        let place = match kind {
            OwnedKind::VisionChat => 0,
            OwnedKind::Work => 1,
        };
        assert_eq!(place, at);
    }
    same(&kinds.to_vec(), "owned_kinds");
    let modes = [ApprovalMode::Yolo, ApprovalMode::Auto, ApprovalMode::Custom];
    for (at, mode) in modes.iter().enumerate() {
        let place = match mode {
            ApprovalMode::Yolo => 0,
            ApprovalMode::Auto => 1,
            ApprovalMode::Custom => 2,
        };
        assert_eq!(place, at);
    }
    same(&modes.to_vec(), "approval_modes");
}

/// A channel page, a `visions-changed` payload and a chat's question (`list_questions`), with
/// every field set.
#[test]
fn pages_changes_and_questions_match_the_apps_fixture() {
    let page = ChannelPage {
        entries: vec![line(
            7,
            ChannelEntry::Person {
                text: "Thanks".into(),
            },
        )],
        next_before: Some(7),
    };
    same(&page, "page");
    let change = crate::types::VisionsChange {
        channel_id: Some("c1".into()),
        vision_id: Some("v1".into()),
    };
    same(&change, "change");
    let question = crate::types::Question {
        id: "q2".into(),
        run_id: "run1".into(),
        question: "Drop Windows?".into(),
        choices: vec!["Yes".into(), "No".into()],
        assumption: Some("No".into()),
        reasons: BTreeMap::from([("No".into(), "Keeps users".into())]),
        group: Some("g1".into()),
        blocking: true,
        topic: Some("platforms".into()),
        subagent: Some("s1".into()),
        asked_at: 5000,
        answer: Some("No".into()),
        answered_by: Some("v1".into()),
        permission: true,
        passed_on: true,
    };
    same(&question, "question");
}
