//! Routines: a chief's standing instruction on a weekly schedule, fired by the Visions tick as if
//! the person had written it; quiet hours hold routines and other agent-started wakes, never the
//! person's own messages.

use std::path::Path;
use std::sync::Arc;

use chrono::{Local, NaiveDate, TimeZone};
use themis_core::configuration::ApprovalMode;
use themis_core::diagnostics::DiagnosticFilter;
use themis_core::visions::Autonomy;

use super::routines::{Routine, Schedule};
use super::*;
use crate::sink::{ChannelSink, EventSink};
use crate::state::AppState;

fn sink() -> Arc<dyn EventSink> {
    Arc::new(ChannelSink::channel().0)
}

/// Local time on a day of November 2026 (the 16th is a Monday).
fn at(day: u32, hour: u32, minute: u32) -> chrono::DateTime<Local> {
    let naive = NaiveDate::from_ymd_opt(2026, 11, day)
        .unwrap()
        .and_hms_opt(hour, minute, 0)
        .unwrap();
    Local.from_local_datetime(&naive).earliest().unwrap()
}

struct Team {
    _dir: tempfile::TempDir,
    state: AppState,
    channel: String,
    juno: String,
    rook: String,
}

fn profile(name: &str) -> Vision {
    Vision {
        id: String::new(),
        name: name.into(),
        role: String::new(),
        soul: String::new(),
        shape: "cloud".into(),
        color: "teal".into(),
        accessory: "none".into(),
        model: None,
        paused: false,
        revision: String::new(),
    }
}

/// `# release`, led by Juno (Rook works there), at `autonomy`.
fn team(autonomy: Autonomy) -> Team {
    let dir = tempfile::tempdir().unwrap();
    let root: &Path = &dir.path().join("project");
    std::fs::create_dir_all(root).unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    // A team's level is never above Settings › Visions: let Settings allow "most".
    std::fs::write(
        dir.path().join("runtime.jsonc"),
        r#"{"visions": {"autonomy": "most"}}"#,
    )
    .unwrap();
    *state.inner.go_catalog.try_write().unwrap() = vec![themis_core::providers::GoModel {
        id: "free".into(),
        price: Some(themis_core::providers::ModelPrice::default()),
        ..Default::default()
    }];
    let juno = state.vision_save(profile("Juno"), None).unwrap().id;
    let rook = state.vision_save(profile("Rook"), None).unwrap().id;
    let channel = state
        .channel_save(
            Channel {
                id: String::new(),
                name: "release".into(),
                topic: String::new(),
                project_root: root.canonicalize().unwrap(),
                approval_mode: ApprovalMode::Custom,
                members: vec![juno.clone(), rook.clone()],
                chief: juno.clone(),
                autonomy: Some(autonomy),
                custom: None,
                daily_budget_usd: None,
                paused_workers: Vec::new(),
                revision: String::new(),
            },
            None,
        )
        .unwrap()
        .id;
    Team {
        _dir: dir,
        state,
        channel,
        juno,
        rook,
    }
}

fn routine(t: &Team, days: &[u8], time: &str, instruction: &str) -> Routine {
    Routine {
        id: String::new(),
        chief: t.juno.clone(),
        channel_id: t.channel.clone(),
        schedule: Schedule {
            days: days.to_vec(),
            time: time.into(),
        },
        instruction: instruction.into(),
        enabled: true,
        next_due_ms: 0,
    }
}

/// Settings › Visions' block (with "most" calls allowed, as in `team`) plus `extra`.
fn runtime(t: &Team, extra: &str) {
    let path = t
        .state
        .inner
        .settings
        .path()
        .with_file_name("runtime.jsonc");
    let mut visions: serde_json::Value = serde_json::from_str(extra).unwrap();
    visions["visions"]["autonomy"] = serde_json::json!("most");
    std::fs::write(path, visions.to_string()).unwrap();
}

/// The wakes the log recorded, as (Vision, reason, outcome).
fn wakes(state: &AppState) -> Vec<(String, String, String)> {
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
            (field("vision_id"), field("reason"), field("outcome"))
        })
        .collect()
}

fn routine_wakes(state: &AppState) -> usize {
    wakes(state)
        .iter()
        .filter(|(_, reason, _)| reason == "routine")
        .count()
}

#[test]
fn a_chief_adds_a_routine_within_its_autonomy_and_says_so() {
    let t = team(Autonomy::Most);
    let saved = t
        .state
        .visions_save_routine(
            &t.channel,
            &t.juno,
            routine(&t, &[1, 2, 3, 4, 5], "09:00", "Summarise yesterday"),
            at(16, 8, 0),
        )
        .unwrap();
    assert!(!saved.id.is_empty());
    let lines = t.state.channel_page(&t.channel, None, 1 << 20).unwrap();
    let set: Vec<_> = lines
        .entries
        .iter()
        .filter(|line| matches!(line.entry, ChannelEntry::RoutineSet { .. }))
        .collect();
    assert_eq!(set.len(), 1, "one line in the channel");
    // At "small" calls, adding work on a schedule is the person's.
    let small = team(Autonomy::Small);
    let refused = small.state.visions_save_routine(
        &small.channel,
        &small.juno,
        routine(&small, &[1], "09:00", "x"),
        at(16, 8, 0),
    );
    assert!(refused.is_err());
    // Only the chief adds routines.
    assert!(t
        .state
        .visions_save_routine(
            &t.channel,
            &t.rook,
            routine(&t, &[1], "09:00", "x"),
            at(16, 8, 0)
        )
        .is_err());
}

#[test]
fn routine_limits_hold() {
    let t = team(Autonomy::Most);
    runtime(&t, r#"{"visions": {"routines_per_chief": 2}}"#);
    let save = |r| {
        t.state
            .visions_save_routine(&t.channel, &t.juno, r, at(16, 8, 0))
    };
    assert!(save(routine(&t, &[1], "09:00", "one")).is_ok());
    assert!(save(routine(&t, &[2], "09:00", "two")).is_ok());
    assert!(
        save(routine(&t, &[3], "09:00", "three")).is_err(),
        "at most two"
    );
    let fresh = team(Autonomy::Most);
    let save = |r| {
        fresh
            .state
            .visions_save_routine(&fresh.channel, &fresh.juno, r, at(16, 8, 0))
    };
    assert!(save(routine(&fresh, &[], "09:00", "no days")).is_err());
    assert!(save(routine(&fresh, &[7], "09:00", "no such day")).is_err());
    assert!(save(routine(&fresh, &[1], "9am", "bad time")).is_err());
    assert!(save(routine(&fresh, &[1], "09:00", "  ")).is_err());
}

#[test]
fn missed_routines_fire_once_at_the_next_allowed_moment() {
    let t = team(Autonomy::Most);
    t.state.set_scheduler_sink(sink());
    // Every day at 09:00, saved on Monday the 16th at 08:00.
    t.state
        .visions_save_routine(
            &t.channel,
            &t.juno,
            routine(&t, &[0, 1, 2, 3, 4, 5, 6], "09:00", "Summarise"),
            at(16, 8, 0),
        )
        .unwrap();
    t.state.visions_routines_tick(&sink(), at(16, 8, 30));
    assert_eq!(routine_wakes(&t.state), 0, "not due yet");
    // The app was closed until Thursday 10:00: three fires were missed, one happens.
    t.state.visions_routines_tick(&sink(), at(19, 10, 0));
    t.state.visions_routines_tick(&sink(), at(19, 10, 1));
    assert_eq!(routine_wakes(&t.state), 1);
    // Next on Friday at 09:00.
    t.state.visions_routines_tick(&sink(), at(20, 8, 59));
    assert_eq!(routine_wakes(&t.state), 1);
    t.state.visions_routines_tick(&sink(), at(20, 9, 0));
    assert_eq!(routine_wakes(&t.state), 2);
}

#[test]
fn quiet_hours_hold_routines_but_not_the_persons_messages() {
    let t = team(Autonomy::Most);
    t.state.set_scheduler_sink(sink());
    runtime(
        &t,
        r#"{"visions": {"quiet_hours": {"start": "22:00", "end": "07:00"}}}"#,
    );
    t.state
        .visions_save_routine(
            &t.channel,
            &t.juno,
            routine(&t, &[0, 1, 2, 3, 4, 5, 6], "06:00", "Early check"),
            at(16, 5, 0),
        )
        .unwrap();
    t.state.visions_routines_tick(&sink(), at(16, 6, 0));
    assert_eq!(routine_wakes(&t.state), 0, "held in quiet hours");
    t.state.visions_routines_tick(&sink(), at(16, 7, 0));
    assert_eq!(routine_wakes(&t.state), 1, "fires once quiet hours end");
    // In quiet hours a worker's task waits, but the person's message wakes the chief.
    t.state.visions_set_clock_for_test(Some(at(16, 23, 0)));
    let tasked = t
        .state
        .visions_wake(&sink(), &t.channel, &t.rook, WakeReason::Tasked(1));
    assert_eq!(tasked, "quiet");
    let (_, woken) = t
        .state
        .visions_post(&sink(), &t.channel, "are you there?")
        .unwrap();
    assert!(
        woken.contains(&t.juno)
            || wakes(&t.state)
                .iter()
                .any(|(v, r, _)| v == &t.juno && r == "mentioned")
    );
}

#[test]
fn the_person_can_pause_and_remove_any_routine() {
    let t = team(Autonomy::Most);
    t.state.set_scheduler_sink(sink());
    let saved = t
        .state
        .visions_save_routine(
            &t.channel,
            &t.juno,
            routine(&t, &[0, 1, 2, 3, 4, 5, 6], "09:00", "Summarise"),
            at(16, 8, 0),
        )
        .unwrap();
    t.state
        .visions_set_routine_enabled(&saved.id, false)
        .unwrap();
    t.state.visions_routines_tick(&sink(), at(16, 9, 0));
    assert_eq!(routine_wakes(&t.state), 0, "paused");
    assert_eq!(t.state.visions_routines(&t.channel).len(), 1);
    t.state.visions_delete_routine(&saved.id).unwrap();
    assert!(t.state.visions_routines(&t.channel).is_empty());
}
