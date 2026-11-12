use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use themis_core::configuration::ApprovalMode;
use themis_core::context_metrics::UsageKind;
use themis_core::providers::{GoModel, ModelPrice};

use super::super::wakes::Admit;
use super::super::{Channel, OwnedKind, WakeReason};
use super::*;
use crate::sink::{ChannelSink, EventSink};
use crate::types::{GoalStatus, ThreadEvent};

/// $1 per million input tokens.
const PAID: &str = "paid-model";

struct Setup {
    dir: tempfile::TempDir,
    state: AppState,
    channel: String,
    wren: String,
    ada: String,
}

fn profile(name: &str) -> Vision {
    Vision {
        id: String::new(),
        name: name.into(),
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
    }
}

fn priced() -> Vec<GoModel> {
    vec![GoModel {
        id: PAID.into(),
        price: Some(ModelPrice {
            input: 1.0,
            output: 0.0,
            ..ModelPrice::default()
        }),
        ..GoModel::default()
    }]
}

/// Wren and Ada in `#hq`, and the paid model priced in the catalog.
async fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let wren = state.vision_save(profile("Wren"), None).unwrap().id;
    let ada = state.vision_save(profile("Ada"), None).unwrap().id;
    let channel = state
        .channel_save(
            Channel {
                id: String::new(),
                name: "hq".into(),
                topic: String::new(),
                project_root: project.canonicalize().unwrap(),
                approval_mode: ApprovalMode::Custom,
                members: vec![wren.clone(), ada.clone()],
                revision: String::new(),
            },
            None,
        )
        .unwrap()
        .id;
    state.set_model_catalog_for_test(priced()).await;
    Setup {
        dir,
        state,
        channel,
        wren,
        ada,
    }
}

fn own(s: &Setup, thread: &str, vision: &str, kind: OwnedKind, hold: Hold) {
    s.state
        .owner_set(Owner {
            thread_id: thread.into(),
            vision_id: vision.into(),
            channel_id: s.channel.clone(),
            kind,
            hold,
        })
        .unwrap();
}

fn row(thread: &str, model: &str, input: u64, at_ms: i64) -> UsageRow {
    UsageRow {
        at_ms,
        provider: themis_core::providers::GO_PROVIDER_ID.into(),
        model: model.into(),
        kind: UsageKind::Answer,
        thread_id: Some(thread.into()),
        run_id: Some("run".into()),
        input_tokens: Some(input),
        cached_input_tokens: None,
        output_tokens: Some(10),
        reasoning_tokens: None,
    }
}

/// Records `rows` and waits for the usage writer to store them.
fn record(state: &AppState, rows: Vec<UsageRow>) {
    let before = state.inner.usage.rows_since(0).unwrap().len();
    let wanted = before + rows.len();
    for row in rows {
        state.inner.usage.record(row);
    }
    let started = Instant::now();
    while state.inner.usage.rows_since(0).unwrap().len() < wanted {
        assert!(started.elapsed() < Duration::from_secs(10), "rows stored");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Wren: its Vision chat, a work chat and a chat the person took over; Ada: a work chat; and a
/// plain chat. Wren spent $0.70 today plus one unpriced request, Ada $1, the plain chat $5.
fn spent_today(s: &Setup) {
    own(s, "wren-chat", &s.wren, OwnedKind::VisionChat, Hold::None);
    own(s, "wren-work", &s.wren, OwnedKind::Work, Hold::None);
    own(s, "wren-mine", &s.wren, OwnedKind::Work, Hold::TakenOver);
    own(s, "ada-work", &s.ada, OwnedKind::Work, Hold::None);
    let now = now_ms();
    record(
        &s.state,
        vec![
            row("wren-chat", PAID, 100_000, now),
            row("wren-work", PAID, 200_000, now),
            row("wren-mine", PAID, 400_000, now),
            row("wren-work", "no-price", 9_000_000, now),
            row("wren-work", PAID, 10_000_000, midnight_ms() - 3_600_000),
            row("ada-work", PAID, 1_000_000, now),
            row("plain", PAID, 5_000_000, now),
        ],
    );
}

fn near(left: f64, right: f64) -> bool {
    (left - right).abs() < 1e-9
}

fn write_runtime(s: &Setup, text: &str) {
    let path = s.dir.path().join("runtime.jsonc");
    std::fs::write(path, text).unwrap();
}

fn set_cap(s: &Setup, vision: &str, cap: Option<f64>) {
    let current = s.state.vision_get(vision).unwrap();
    let revision = current.revision.clone();
    s.state
        .vision_save(
            Vision {
                daily_budget_usd: cap,
                ..current
            },
            Some(&revision),
        )
        .unwrap();
}

/// The `budget_reached` notices in the channel.
fn budget_notices(s: &Setup) -> Vec<Value> {
    let page = s.state.channel_page(&s.channel, None, 1 << 20).unwrap();
    page.entries
        .into_iter()
        .map(|line| serde_json::to_value(line.entry).unwrap())
        .filter(|entry| entry["type"] == "notice" && entry["kind"] == "budget_reached")
        .collect()
}

fn running(state: &AppState, thread: &str) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    state
        .inner
        .stop_flags
        .lock()
        .unwrap()
        .insert(thread.into(), Arc::clone(&flag));
    flag
}

fn sink() -> Arc<dyn EventSink> {
    Arc::new(ChannelSink::channel().0)
}

fn finished() -> ThreadEvent {
    ThreadEvent::Finished {
        model: None,
        result: "done".into(),
    }
}

/// Spend counts today's requests of the chats each Vision owns (its Vision chat, its work
/// chats, one the person took over), never a plain chat's or yesterday's; an unpriced request
/// counts $0 and is counted apart.
#[tokio::test]
async fn spend_counts_only_owned_chats_since_midnight() {
    let s = setup().await;
    spent_today(&s);
    assert!(near(s.state.visions_spent_today(Some(&s.wren)), 0.7));
    assert!(near(s.state.visions_spent_today(Some(&s.ada)), 1.0));
    assert!(near(s.state.visions_spent_today(None), 1.7));
    let tally = s.state.visions_tally(Some(&s.wren));
    assert_eq!(tally.vision.unpriced, 1);
    assert_eq!(tally.all.unpriced, 1);
    // The default overall cap ($1) is reached by both together.
    assert!(s.state.visions_over_budget(&s.wren));
    write_runtime(&s, r#"{"visions": {"daily_budget_usd": 5}}"#);
    assert!(!s.state.visions_over_budget(&s.wren));
    set_cap(&s, &s.wren, Some(0.5));
    assert!(s.state.visions_over_budget(&s.wren));
    assert!(!s.state.visions_over_budget(&s.ada));
}

/// At a run's end over budget, the Vision's running chats are stopped, except one the person
/// took over, and nothing is held back; another Vision's chats run on. Its channel gets one
/// notice a day, with the unpriced count, even after a restart.
#[tokio::test]
async fn reaching_the_cap_stops_runs_and_posts_once() {
    let s = setup().await;
    spent_today(&s);
    write_runtime(&s, r#"{"visions": {"daily_budget_usd": 5}}"#);
    let work = running(&s.state, "wren-work");
    let mine = running(&s.state, "wren-mine");
    let theirs = running(&s.state, "ada-work");
    s.state.visions_after_run(&sink(), "plain", &finished());
    assert!(budget_notices(&s).is_empty(), "a plain chat's end");
    s.state.visions_after_run(&sink(), "wren-chat", &finished());
    assert!(budget_notices(&s).is_empty(), "under the cap");

    set_cap(&s, &s.wren, Some(0.5));
    s.state.visions_after_run(&sink(), "wren-chat", &finished());
    assert!(work.load(Ordering::SeqCst), "its work chat is stopped");
    assert!(!mine.load(Ordering::SeqCst), "the person's chat runs on");
    assert!(
        !theirs.load(Ordering::SeqCst),
        "another Vision's chat runs on"
    );
    assert_eq!(
        s.state.owner_of("wren-work").unwrap().hold,
        Hold::None,
        "the budget's stop holds nothing back"
    );
    let notices = budget_notices(&s);
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0]["vision_id"], Value::String(s.wren.clone()));
    assert_eq!(notices[0]["unpriced_requests"], 1);

    s.state.visions_after_run(&sink(), "wren-work", &finished());
    assert_eq!(budget_notices(&s).len(), 1, "once a day");
    let restarted = AppState::new_for_test(s.dir.path().join("settings.json"));
    restarted.set_model_catalog_for_test(priced()).await;
    restarted.visions_after_run(&sink(), "wren-work", &finished());
    assert_eq!(budget_notices(&s).len(), 1, "once a day across a restart");
}

/// Over budget a Vision wakes for nothing, and what woke it waits; raising its profile's cap
/// lets the next wake run. A profile's cap may only lower the overall one: raising it past
/// that changes nothing until the overall cap is raised.
#[tokio::test]
async fn raising_the_cap_lets_the_next_wake_run() {
    let s = setup().await;
    spent_today(&s);
    write_runtime(&s, r#"{"visions": {"daily_budget_usd": 5}}"#);
    set_cap(&s, &s.wren, Some(0.5));
    let admit = |reason| s.state.visions_wake_admit(&s.channel, &s.wren, reason);
    assert_eq!(admit(Some(WakeReason::Mentioned)), Err("over_budget"));

    set_cap(&s, &s.wren, Some(2.0));
    assert_eq!(
        admit(None),
        Ok(Admit::Start(vec![WakeReason::Mentioned])),
        "what woke it waited"
    );
    s.state.visions_cache().wakes.ended(&s.wren, &s.channel);

    set_cap(&s, &s.wren, Some(50.0));
    write_runtime(&s, r#"{"visions": {"daily_budget_usd": 1}}"#);
    assert_eq!(admit(Some(WakeReason::Mentioned)), Err("over_budget"));
    write_runtime(&s, r#"{"visions": {"daily_budget_usd": 5}}"#);
    assert_eq!(admit(None), Ok(Admit::Start(vec![WakeReason::Mentioned])));
}

/// A goal in a chat whose Vision is over budget pauses instead of starting its next turn; in a
/// chat the person took over it is theirs and goes on being decided as before.
#[tokio::test]
async fn a_goal_in_an_over_budget_chat_pauses() {
    let s = setup().await;
    set_cap(&s, &s.wren, Some(0.0));
    let project = s.dir.path().join("project");
    let thread = s
        .state
        .create_thread(
            project.to_string_lossy().into_owned(),
            crate::types::ProviderKind::Go,
            Some(PAID.into()),
        )
        .await
        .unwrap()
        .id;
    own(&s, &thread, &s.wren, OwnedKind::Work, Hold::None);
    let sink = sink();
    s.state.begin_goal(sink.as_ref(), &thread, "ship it");
    s.state
        .start_goal_turn(Arc::clone(&sink), thread.clone(), String::new())
        .await;
    let goal = s.state.goal_for(&thread).unwrap();
    assert_eq!(goal.status, GoalStatus::Paused);
    assert!(!s.state.get_thread(&thread).await.unwrap().running);
    assert!(s.state.visions_budget_holds(&thread));
    own(&s, &thread, &s.wren, OwnedKind::Work, Hold::TakenOver);
    assert!(!s.state.visions_budget_holds(&thread));
}

/// Scale: today's spend reads only today's rows and looks each one up once, however long the
/// history and however many chats Visions own.
#[tokio::test]
async fn spend_reads_only_todays_rows_however_long_the_history() {
    let mut examined = Vec::new();
    for size in [20usize, 400] {
        let s = setup().await;
        let yesterday = midnight_ms() - 3_600_000;
        let mut rows = Vec::new();
        for n in 0..size {
            let thread = format!("old-{n}");
            own(&s, &thread, &s.wren, OwnedKind::Work, Hold::None);
            rows.push(row(&thread, PAID, 1_000, yesterday));
        }
        own(&s, "today", &s.wren, OwnedKind::Work, Hold::None);
        for _ in 0..5 {
            rows.push(row("today", PAID, 1_000_000, now_ms()));
        }
        record(&s.state, rows);
        let tally = s.state.visions_tally(Some(&s.wren));
        assert!(near(tally.vision.usd, 5.0));
        examined.push(tally.examined);
    }
    assert_eq!(examined, vec![5, 5], "flat in history and owned chats");
}
