//! Requests between channels: a chief asks another channel's chief, which accepts or declines
//! within its channel's level; both channels show the exchange; a daily cap stops loops.

use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use themis_core::configuration::ApprovalMode;
use themis_core::diagnostics::DiagnosticFilter;
use themis_core::visions::Autonomy;

use super::*;
use crate::sink::{ChannelSink, EventSink};
use crate::state::AppState;

struct Company {
    _dir: tempfile::TempDir,
    state: AppState,
    /// (channel, chief) for `# release` and `# api`, in one project.
    release: (String, String),
    api: (String, String),
}

fn sink() -> Arc<dyn EventSink> {
    Arc::new(ChannelSink::channel().0)
}

fn lead(state: &AppState, root: &Path, name: &str, chief: &str) -> (String, String) {
    let vision = state
        .vision_save(
            Vision {
                id: String::new(),
                name: chief.into(),
                role: String::new(),
                soul: String::new(),
                shape: "cloud".into(),
                color: "teal".into(),
                accessory: "none".into(),
                model: None,
                paused: false,
                revision: String::new(),
            },
            None,
        )
        .unwrap()
        .id;
    let channel = state
        .channel_save(
            Channel {
                id: String::new(),
                name: name.into(),
                topic: String::new(),
                project_root: root.to_path_buf(),
                approval_mode: ApprovalMode::Custom,
                members: vec![vision.clone()],
                chief: vision.clone(),
                autonomy: Some(Autonomy::Most),
                custom: None,
                daily_budget_usd: None,
                paused_workers: Vec::new(),
                revision: String::new(),
            },
            None,
        )
        .unwrap()
        .id;
    (channel, vision)
}

fn company() -> Company {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    // Budgets need prices; a free model is enough.
    *state.inner.go_catalog.try_write().unwrap() = vec![themis_core::providers::GoModel {
        id: "free".into(),
        price: Some(themis_core::providers::ModelPrice::default()),
        ..Default::default()
    }];
    let release = lead(&state, &root, "release", "Juno");
    let api = lead(&state, &root, "api", "Tess");
    Company {
        _dir: dir,
        state,
        release,
        api,
    }
}

/// The wakes the log recorded, as (Vision, channel, reason, outcome).
fn wakes(state: &AppState) -> Vec<(String, String, String, String)> {
    let filter = DiagnosticFilter {
        service: Some("visions".into()),
        ..DiagnosticFilter::default()
    };
    let mut records = state.inner.diagnostics.query(&filter).unwrap();
    records.sort_by_key(|record| record.timestamp);
    records
        .into_iter()
        .filter(|record| record.event == "wake")
        .map(|record| {
            let field = |key: &str| record.details[key].as_str().unwrap_or_default().to_owned();
            (
                field("vision_id"),
                field("channel_id"),
                field("reason"),
                field("outcome"),
            )
        })
        .collect()
}

fn lines(state: &AppState, channel: &str) -> Vec<Value> {
    let page = state.channel_page(channel, None, 1 << 20).unwrap();
    page.entries
        .into_iter()
        .map(|line| serde_json::to_value(line.entry).unwrap())
        .collect()
}

fn of_type<'a>(lines: &'a [Value], kind: &str) -> Vec<&'a Value> {
    lines.iter().filter(|line| line["type"] == kind).collect()
}

#[test]
fn a_request_wakes_the_other_chief_and_shows_in_both_channels() {
    let c = company();
    let (release, juno) = &c.release;
    let (api, tess) = &c.api;
    let id = c
        .state
        .visions_request(
            &sink(),
            release,
            juno,
            api,
            "Fix the export date format before Friday",
        )
        .unwrap();
    let sent = lines(&c.state, release);
    let sent = of_type(&sent, "request_sent");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["request_id"], id.as_str());
    assert_eq!(sent[0]["channel_id"], api.as_str());
    let received = lines(&c.state, api);
    let received = of_type(&received, "request_received");
    assert_eq!(received.len(), 1);
    assert_eq!(received[0]["channel_id"], release.as_str());
    assert_eq!(
        received[0]["ask"],
        "Fix the export date format before Friday"
    );
    let woke = wakes(&c.state);
    assert!(
        woke.iter()
            .any(|(vision, channel, reason, _)| vision == tess
                && channel == api
                && reason == "request"),
        "{woke:?}"
    );
}

#[test]
fn a_chief_declines_within_its_autonomy() {
    let c = company();
    let (release, juno) = &c.release;
    let (api, tess) = &c.api;
    // At "ask", # api's chief decides nothing: it may decline, but not take on the work.
    let mut channel = c.state.channel_get(api).unwrap();
    channel.autonomy = Some(Autonomy::Ask);
    let revision = channel.revision.clone();
    c.state.channel_save(channel, Some(&revision)).unwrap();
    let first = c
        .state
        .visions_request(&sink(), release, juno, api, "Rename the CLI")
        .unwrap();
    let refused = c
        .state
        .visions_answer_request(&sink(), api, tess, &first, true, "on it");
    assert!(refused.is_err(), "accepting is beyond its level");
    c.state
        .visions_answer_request(&sink(), api, tess, &first, false, "Not this week")
        .unwrap();
    // Answered once only, and both channels show the answer.
    assert!(c
        .state
        .visions_answer_request(&sink(), api, tess, &first, false, "again")
        .is_err());
    for channel in [release, api] {
        let all = lines(&c.state, channel);
        let answered = of_type(&all, "request_answered");
        assert_eq!(answered.len(), 1, "{all:?}");
        assert_eq!(answered[0]["accepted"], false);
    }
    // The asking chief hears the answer.
    let woke = wakes(&c.state);
    assert!(woke
        .iter()
        .any(|(vision, _, reason, _)| vision == juno && reason == "request_answered"));
    // Only the receiving chief answers.
    let second = c
        .state
        .visions_request(&sink(), release, juno, api, "Add a flag")
        .unwrap();
    assert!(c
        .state
        .visions_answer_request(&sink(), release, juno, &second, false, "no")
        .is_err());
    // The person may accept what the chief could not.
    c.state
        .visions_person_answers_request(&sink(), &second, true)
        .unwrap();
}

#[test]
fn requests_between_channels_stop_at_the_daily_cap() {
    let c = company();
    let (release, juno) = &c.release;
    let (api, _) = &c.api;
    let runtime = c
        .state
        .inner
        .settings
        .path()
        .with_file_name("runtime.jsonc");
    std::fs::write(&runtime, r#"{"visions": {"requests_per_day": 2}}"#).unwrap();
    for ask in ["one", "two"] {
        c.state
            .visions_request(&sink(), release, juno, api, ask)
            .unwrap();
    }
    for ask in ["three", "four"] {
        let refused = c
            .state
            .visions_request(&sink(), release, juno, api, ask)
            .unwrap_err();
        assert!(refused.contains("requests"), "{refused}");
    }
    let notices = lines(&c.state, release)
        .into_iter()
        .filter(|line| line["type"] == "notice" && line["kind"] == "request_cap")
        .count();
    assert_eq!(notices, 1, "told once");
    // The cap is the sending channel's: # api may still ask # release.
    let (api, tess) = &c.api;
    assert!(c
        .state
        .visions_request(&sink(), api, tess, release, "help")
        .is_ok());
}

#[test]
fn a_request_to_another_project_is_refused() {
    let c = company();
    let (release, juno) = &c.release;
    let other = c._dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let (far, _) = lead(&c.state, &other.canonicalize().unwrap(), "far", "Ada");
    assert!(c
        .state
        .visions_request(&sink(), release, juno, &far, "x")
        .is_err());
    // Nor to itself, nor to # administration (Atlas is asked for workers another way).
    assert!(c
        .state
        .visions_request(&sink(), release, juno, release, "x")
        .is_err());
}

#[test]
fn the_receiving_channel_pays_for_a_request() {
    let c = company();
    let (release, juno) = &c.release;
    let (api, tess) = &c.api;
    // # api has no budget left today; # release has room.
    let mut channel = c.state.channel_get(api).unwrap();
    channel.daily_budget_usd = Some(0.0);
    let revision = channel.revision.clone();
    c.state.channel_save(channel, Some(&revision)).unwrap();
    c.state
        .visions_request(&sink(), release, juno, api, "Fix it")
        .unwrap();
    let woke = wakes(&c.state);
    let wake = woke
        .iter()
        .find(|(vision, channel, reason, _)| {
            vision == tess && channel == api && reason == "request"
        })
        .expect("a wake was tried");
    assert_eq!(
        wake.3, "over_budget",
        "the wake is the receiving channel's to pay"
    );
    assert!(
        !c.state.visions_over_budget(juno),
        "the asking channel is not charged"
    );
}

/// The person accepts what their chief could not: the answer is theirs (no Vision), and the
/// receiving chief wakes to do the work.
#[test]
fn the_persons_accept_wakes_the_receiving_chief() {
    let c = company();
    let (release, juno) = &c.release;
    let (api, tess) = &c.api;
    let id = c
        .state
        .visions_request(&sink(), release, juno, api, "Ship it")
        .unwrap();
    c.state
        .visions_person_answers_request(&sink(), &id, true)
        .unwrap();
    let all = lines(&c.state, api);
    let answered = of_type(&all, "request_answered");
    assert_eq!(answered[0]["vision_id"], "", "the person's answer");
    let woke = wakes(&c.state);
    assert!(
        woke.iter()
            .any(|(vision, channel, reason, _)| vision == tess
                && channel == api
                && reason == "request_answered"),
        "{woke:?}"
    );
}

/// Requests nobody can answer any more (their channel is gone) are dropped from the store.
#[test]
fn requests_to_a_gone_channel_are_dropped() {
    let c = company();
    let (release, juno) = &c.release;
    let (api, _) = &c.api;
    let id = c
        .state
        .visions_request(&sink(), release, juno, api, "Ship it")
        .unwrap();
    c.state.visions_delete_channel(&sink(), api).unwrap();
    let other = c._dir.path().join("project").canonicalize().unwrap();
    let (docs, _) = lead(&c.state, &other, "docs", "Ada");
    c.state
        .visions_request(&sink(), release, juno, &docs, "Write it up")
        .unwrap();
    let stored = std::fs::read_to_string(c.state.inner.visions_dir.join("requests.json")).unwrap();
    assert!(!stored.contains(&id), "{stored}");
}

/// A cap notice the channel could not record is posted on a later refused call.
#[test]
fn a_cap_notice_that_failed_is_tried_again() {
    let c = company();
    let (release, juno) = &c.release;
    let (api, _) = &c.api;
    let runtime = c
        .state
        .inner
        .settings
        .path()
        .with_file_name("runtime.jsonc");
    std::fs::write(&runtime, r#"{"visions": {"requests_per_day": 1}}"#).unwrap();
    c.state
        .visions_request(&sink(), release, juno, api, "one")
        .unwrap();
    let log = c
        .state
        .inner
        .visions_dir
        .join("channels")
        .join(format!("{release}.jsonl"));
    let kept = std::fs::read(&log).unwrap();
    std::fs::remove_file(&log).unwrap();
    std::fs::create_dir_all(&log).unwrap();
    assert!(c
        .state
        .visions_request(&sink(), release, juno, api, "two")
        .is_err());
    std::fs::remove_dir(&log).unwrap();
    std::fs::write(&log, kept).unwrap();
    c.state.visions_cache().budget.retry_after.clear();
    assert!(c
        .state
        .visions_request(&sink(), release, juno, api, "three")
        .is_err());
    let notices = lines(&c.state, release)
        .into_iter()
        .filter(|line| line["type"] == "notice" && line["kind"] == "request_cap")
        .count();
    assert_eq!(notices, 1);
}
