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
use crate::usage::UsageRow;

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

/// A free model: priced, at $0.
const FREE: &str = "free-model";

fn priced() -> Vec<GoModel> {
    vec![
        GoModel {
            id: PAID.into(),
            price: Some(ModelPrice {
                input: 1.0,
                output: 0.0,
                ..ModelPrice::default()
            }),
            ..GoModel::default()
        },
        GoModel {
            id: FREE.into(),
            price: Some(ModelPrice::default()),
            ..GoModel::default()
        },
        // Listed without a price.
        GoModel {
            id: "listed-only".into(),
            ..GoModel::default()
        },
    ]
}

fn midnight() -> i64 {
    midnight_ms(chrono::Local::now().date_naive())
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
            row("wren-work", PAID, 10_000_000, midnight() - 3_600_000),
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
    let tally = s.state.visions_tally(Some(&s.wren)).unwrap();
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
    assert!(
        restarted.visions_over_budget(&s.wren),
        "rebuilt from the store"
    );
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
    // The goal bar shows "Paused · " and the reason.
    assert_eq!(
        goal.reason.as_deref(),
        Some("its Vision reached today's budget")
    );
    assert!(!s.state.get_thread(&thread).await.unwrap().running);
    assert!(s.state.visions_budget_holds(&thread).is_some());
    own(&s, &thread, &s.wren, OwnedKind::Work, Hold::TakenOver);
    assert!(s.state.visions_budget_holds(&thread).is_none());
}

/// Scale: a check reads only the rows recorded since the last one and sums per chat that spent
/// today, however long the history, however many requests today and however many chats Visions
/// own. The sums are rebuilt from the store after a restart.
#[tokio::test]
async fn spend_reads_only_new_rows_however_long_the_history_and_the_day() {
    let mut read = Vec::new();
    for size in [20usize, 400] {
        let s = setup().await;
        let yesterday = midnight() - 3_600_000;
        let mut rows = Vec::new();
        for n in 0..size {
            let thread = format!("old-{n}");
            own(&s, &thread, &s.wren, OwnedKind::Work, Hold::None);
            rows.push(row(&thread, PAID, 1_000, yesterday));
        }
        own(&s, "today", &s.wren, OwnedKind::Work, Hold::None);
        for _ in 0..size {
            rows.push(row("today", PAID, 10_000, now_ms()));
        }
        record(&s.state, rows);
        let rows_read = |state: &AppState| state.visions_cache().budget.rows_read;
        let first = s.state.visions_tally(Some(&s.wren)).unwrap();
        assert!(near(first.vision.usd, size as f64 * 0.01));
        let after_first = rows_read(&s.state);
        s.state.visions_tally(Some(&s.wren)).unwrap();
        let again = rows_read(&s.state) - after_first;
        record(&s.state, vec![row("today", PAID, 10_000, now_ms()); 3]);
        let third = s.state.visions_tally(Some(&s.wren)).unwrap();
        assert!(near(third.vision.usd, (size + 3) as f64 * 0.01));
        let new = rows_read(&s.state) - after_first - again;
        read.push((after_first, again, new));
    }
    assert_eq!(
        read,
        vec![(20, 0, 3), (400, 0, 3)],
        "today's rows once, then new rows only"
    );
}

/// The notices of `kind` in the channel.
fn notices_of(s: &Setup, kind: &str) -> Vec<Value> {
    let page = s.state.channel_page(&s.channel, None, 1 << 20).unwrap();
    page.entries
        .into_iter()
        .map(|line| serde_json::to_value(line.entry).unwrap())
        .filter(|entry| entry["type"] == "notice" && entry["kind"] == kind)
        .collect()
}

fn unknown(state: &AppState, vision: &str) -> Option<&'static str> {
    match state.visions_judge(vision).over {
        Some(Over::Unknown(why)) => Some(why.label()),
        _ => None,
    }
}

/// Spend that cannot be known counts as the budget reached, never as $0: no price catalog yet,
/// the usage store unreadable, `owners.json` unreadable, the configuration unreadable. The
/// channel's notice names the reason.
#[tokio::test]
async fn the_budget_fails_closed_on_every_read_error() {
    let s = setup().await;
    s.state.set_model_catalog_for_test(Vec::new()).await;
    assert_eq!(unknown(&s.state, &s.wren), Some("catalog_not_loaded"));
    assert!(s.state.visions_over_budget(&s.wren));
    assert!(s.state.visions_spent_today(Some(&s.wren)).is_infinite());
    assert!(s.state.visions_budget_gate(&s.wren).is_some());
    let told = notices_of(&s, "budget_unknown");
    assert_eq!(told.len(), 1);
    assert_eq!(told[0]["reason"], "catalog_not_loaded");
    assert!(notices_of(&s, "budget_reached").is_empty());
    s.state.set_model_catalog_for_test(priced()).await;
    assert_eq!(s.state.visions_judge(&s.wren).over, None);

    write_runtime(&s, "{ not json");
    assert_eq!(unknown(&s.state, &s.wren), Some("config_unreadable"));
    write_runtime(&s, "{}");

    s.state.inner.usage.break_reads_for_test();
    assert_eq!(unknown(&s.state, &s.wren), Some("usage_unreadable"));

    let damaged = setup().await;
    std::fs::create_dir_all(&damaged.state.inner.visions_dir).unwrap();
    let owners = damaged.state.inner.visions_dir.join("owners.json");
    std::fs::write(owners, "{ damaged").unwrap();
    damaged.state.visions_cache().owners = None;
    assert_eq!(
        unknown(&damaged.state, &damaged.wren),
        Some("owners_unreadable")
    );
}

/// A model the loaded catalog does not price counts as $0 and is told once a day as soon as it
/// happens, before any cap; a free model, priced at $0, is priced.
#[tokio::test]
async fn requests_without_a_price_are_told_once_before_the_cap() {
    let s = setup().await;
    own(&s, "wren-work", &s.wren, OwnedKind::Work, Hold::None);
    record(&s.state, vec![row("wren-work", FREE, 5_000_000, now_ms())]);
    assert_eq!(s.state.visions_budget_gate(&s.wren), None);
    assert!(
        notices_of(&s, "unpriced").is_empty(),
        "a free model is priced"
    );
    record(
        &s.state,
        vec![row("wren-work", "listed-only", 10, now_ms())],
    );
    assert_eq!(s.state.visions_budget_gate(&s.wren), None);
    record(&s.state, vec![row("wren-work", "not-listed", 10, now_ms())]);
    assert_eq!(s.state.visions_budget_gate(&s.wren), None);
    let told = notices_of(&s, "unpriced");
    assert_eq!(told.len(), 1);
    assert_eq!(told[0]["unpriced_requests"], 1);
}

/// The record of a request past the cap stops the Vision's running chats at once, before the
/// run ends; at the overall cap every Vision's. Nothing is held back, and a stopped work chat
/// waits as `budget_stopped` for the Vision's next wake, after the cap is raised.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_past_the_cap_stops_the_running_chats_at_once() {
    let s = setup().await;
    write_runtime(&s, r#"{"visions": {"daily_budget_usd": 5}}"#);
    set_cap(&s, &s.wren, Some(0.5));
    own(&s, "wren-work", &s.wren, OwnedKind::Work, Hold::None);
    own(&s, "wren-other", &s.wren, OwnedKind::Work, Hold::None);
    own(&s, "ada-work", &s.ada, OwnedKind::Work, Hold::None);
    let work = running(&s.state, "wren-work");
    let other = running(&s.state, "wren-other");
    let theirs = running(&s.state, "ada-work");
    let record = s.state.usage_recorder("wren-work", "run-1");
    let request = |input: u64| themis_core::context_metrics::UsageRecord {
        provider: themis_core::providers::GO_PROVIDER_ID.into(),
        model: PAID.into(),
        kind: UsageKind::Answer,
        input_tokens: Some(input),
        cached_input_tokens: None,
        output_tokens: Some(1),
        reasoning_tokens: None,
    };
    record(request(100_000));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!work.load(Ordering::SeqCst), "under the cap");
    record(request(600_000));
    let started = Instant::now();
    while !(work.load(Ordering::SeqCst) && other.load(Ordering::SeqCst)) {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "stopped mid-run"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !theirs.load(Ordering::SeqCst),
        "Wren's own cap stops only Wren's"
    );
    assert_eq!(s.state.owner_of("wren-work").unwrap().hold, Hold::None);

    // At the overall cap every Vision's chats stop.
    write_runtime(&s, r#"{"visions": {"daily_budget_usd": 0.5}}"#);
    record(request(1));
    let started = Instant::now();
    while !theirs.load(Ordering::SeqCst) {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "everyone stopped"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Raised: the next wake says the budget stopped them.
    write_runtime(&s, r#"{"visions": {"daily_budget_usd": 50}}"#);
    set_cap(&s, &s.wren, None);
    let next = s.state.visions_wake_admit(&s.channel, &s.wren, None);
    let Ok(Admit::Start(reasons)) = next else {
        panic!("{next:?}");
    };
    assert!(reasons.contains(&WakeReason::BudgetStopped("wren-work".into())));
    assert!(reasons.contains(&WakeReason::BudgetStopped("wren-other".into())));
}

/// A notice that was posted but cannot be saved is not posted again on every check.
#[tokio::test]
async fn a_notice_that_cannot_be_saved_is_posted_once() {
    let s = setup().await;
    std::fs::create_dir_all(s.state.inner.visions_dir.join("budget_notices.json")).unwrap();
    set_cap(&s, &s.wren, Some(0.0));
    for _ in 0..3 {
        assert!(s.state.visions_budget_gate(&s.wren).is_some());
    }
    assert_eq!(budget_notices(&s).len(), 1);
}

/// A project that lowers `visions.daily_budget_usd` lowers it for both the server's checks and
/// `visions_over_budget`.
#[tokio::test]
async fn a_projects_lower_cap_is_the_one_both_checks_use() {
    let s = setup().await;
    spent_today(&s);
    write_runtime(&s, r#"{"visions": {"daily_budget_usd": 5}}"#);
    assert!(!s.state.visions_over_budget(&s.ada));
    let project = s.dir.path().join("project/.themis");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join("config.jsonc"),
        r#"{"visions": {"daily_budget_usd": 1}}"#,
    )
    .unwrap();
    assert!(s.state.visions_over_budget(&s.ada));
    assert!(s.state.visions_budget_gate(&s.ada).is_some());
}

/// With no price catalog and none to load (no key here), the Vision starts nothing and its
/// channel is told why.
#[tokio::test(flavor = "multi_thread")]
async fn a_catalog_that_cannot_load_is_told() {
    let s = setup().await;
    s.state.set_model_catalog_for_test(Vec::new()).await;
    s.state.visions_prices_at_start();
    let started = Instant::now();
    while notices_of(&s, "budget_unknown").is_empty() {
        assert!(started.elapsed() < Duration::from_secs(10), "told");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        notices_of(&s, "budget_unknown")[0]["reason"],
        "catalog_not_loaded"
    );
    assert_eq!(
        s.state
            .visions_wake_admit(&s.channel, &s.wren, Some(WakeReason::Mentioned)),
        Err("budget_unknown")
    );
}
