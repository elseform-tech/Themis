//! Durable, content-free token usage: one row per completed model request.
//!
//! Rows hold time, provider, model, kind, chat and run ids and provider-reported token
//! counts; never prompts, replies or credentials. They live in their own SQLite file
//! (`usage.sqlite3` in the app data folder) so a usage write never takes the
//! conversation store's write lock: recording is best effort, off the run's path
//! (a bounded queue drained by one writer thread), and a full queue or a failed write
//! drops the row instead of slowing or failing a run.
//!
//! Rows older than [`RAW_RETENTION_DAYS`] are rolled into daily totals per provider,
//! model, kind and chat; daily totals older than [`DAILY_RETENTION_DAYS`] are removed.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::json;
use themis_core::context_metrics::{UsageKind, UsageRecord};
use themis_core::diagnostics::DiagnosticLog;
use themis_core::providers::ModelPrice;

/// Raw rows are kept this long (more than a month, so "this month" and the 30-day
/// chart always read whole requests).
pub const RAW_RETENTION_DAYS: i64 = 35;
/// Daily totals are kept this long.
pub const DAILY_RETENTION_DAYS: i64 = 400;
/// Days in the daily chart.
pub const CHART_DAYS: usize = 30;
/// Rows waiting for the writer; more are dropped.
const QUEUE_BOUND: usize = 1024;
/// The writer rolls old rows up after this many inserts (and when it starts).
const ROLLUP_EVERY: u64 = 500;
/// Longest stored id or model name.
const MAX_ID_CHARS: usize = 128;
/// Chats listed in the breakdown; the rest are summed into one "other chats" row.
const TOP_CHATS: usize = 5;

/// One completed model request as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRow {
    pub at_ms: i64,
    pub provider: String,
    pub model: String,
    pub kind: UsageKind,
    pub thread_id: Option<String>,
    pub run_id: Option<String>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}

impl UsageRow {
    pub fn from_record(
        record: UsageRecord,
        at_ms: i64,
        thread_id: Option<&str>,
        run_id: Option<&str>,
    ) -> Self {
        Self {
            at_ms,
            provider: bounded(&record.provider),
            model: bounded(&record.model),
            kind: record.kind,
            thread_id: thread_id.map(bounded),
            run_id: run_id.map(bounded),
            input_tokens: record.input_tokens,
            cached_input_tokens: record.cached_input_tokens,
            output_tokens: record.output_tokens,
            reasoning_tokens: record.reasoning_tokens,
        }
    }
}

fn bounded(text: &str) -> String {
    text.chars().take(MAX_ID_CHARS).collect()
}

fn kind_name(kind: UsageKind) -> &'static str {
    kind.as_str()
}

fn kind_from(name: &str) -> UsageKind {
    match name {
        "summary" => UsageKind::Summary,
        "goal_check" => UsageKind::GoalCheck,
        _ => UsageKind::Answer,
    }
}

pub struct UsageStore {
    connection: Option<Arc<Mutex<Connection>>>,
    sender: Mutex<Option<SyncSender<UsageRow>>>,
    dropped: Arc<AtomicU64>,
    diagnostics: Arc<DiagnosticLog>,
}

impl UsageStore {
    /// Opens (or creates) the store and starts its writer. Never fails: a store that
    /// cannot open records nothing and reports itself unavailable.
    pub fn open(path: &Path, diagnostics: Arc<DiagnosticLog>) -> Self {
        let connection = match open_connection(path) {
            Ok(connection) => Some(Arc::new(Mutex::new(connection))),
            Err(error) => {
                let _ = diagnostics.append(
                    "usage",
                    "store_unavailable",
                    "warn",
                    json!({"error_kind": error_kind(&error)}),
                );
                None
            }
        };
        let dropped = Arc::new(AtomicU64::new(0));
        let sender = connection.as_ref().and_then(|connection| {
            let (sender, receiver) = sync_channel(QUEUE_BOUND);
            let connection = Arc::clone(connection);
            let log = Arc::clone(&diagnostics);
            std::thread::Builder::new()
                .name("themis-usage".into())
                .spawn(move || write_rows(receiver, connection, log))
                .ok()
                .map(|_| sender)
        });
        Self {
            connection,
            sender: Mutex::new(sender),
            dropped,
            diagnostics,
        }
    }

    pub fn available(&self) -> bool {
        self.connection.is_some()
    }

    /// Queues a row without waiting; a full queue drops it (counted in diagnostics).
    pub fn record(&self, row: UsageRow) {
        let sender = self.sender.lock().unwrap_or_else(|e| e.into_inner());
        let Some(sender) = sender.as_ref() else {
            return;
        };
        if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) = sender.try_send(row) {
            let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            if dropped == 1 || dropped.is_multiple_of(100) {
                let _ = self.diagnostics.append(
                    "usage",
                    "rows_dropped",
                    "warn",
                    json!({"dropped_total": dropped}),
                );
            }
        }
    }

    /// Rows at or after `since_ms`, oldest first.
    pub fn rows_since(&self, since_ms: i64) -> Result<Vec<UsageRow>, String> {
        let Some(connection) = &self.connection else {
            return Ok(Vec::new());
        };
        let connection = connection.lock().unwrap_or_else(|e| e.into_inner());
        read_rows(&connection, since_ms).map_err(|e| format!("read usage: {e}"))
    }

    /// Every request of one chat that Themis still keeps (raw rows plus daily totals).
    pub fn chat_totals(&self, thread_id: &str) -> Result<Totals, String> {
        let Some(connection) = &self.connection else {
            return Ok(Totals::default());
        };
        let connection = connection.lock().unwrap_or_else(|e| e.into_inner());
        connection
            .query_row(
                "SELECT COALESCE(SUM(requests),0),COALESCE(SUM(unreported),0),COALESCE(SUM(input),0),COALESCE(SUM(cached),0),COALESCE(SUM(output),0),COALESCE(SUM(reasoning),0) FROM (
                   SELECT COUNT(*) AS requests,SUM(input_tokens IS NULL AND output_tokens IS NULL) AS unreported,SUM(input_tokens) AS input,SUM(cached_input_tokens) AS cached,SUM(output_tokens) AS output,SUM(reasoning_tokens) AS reasoning FROM usage_requests WHERE thread_id=?1
                   UNION ALL
                   SELECT SUM(requests),SUM(unreported),SUM(input_tokens),SUM(cached_input_tokens),SUM(output_tokens),SUM(reasoning_tokens) FROM usage_daily WHERE thread_id=?1)",
                [thread_id],
                |row| {
                    Ok(Totals {
                        requests: row.get::<_, i64>(0)? as u64,
                        unreported_requests: row.get::<_, i64>(1)? as u64,
                        input_tokens: row.get::<_, i64>(2)? as u64,
                        cached_input_tokens: row.get::<_, i64>(3)? as u64,
                        output_tokens: row.get::<_, i64>(4)? as u64,
                        reasoning_tokens: row.get::<_, i64>(5)? as u64,
                        ..Totals::default()
                    })
                },
            )
            .map_err(|e| format!("read chat usage: {e}"))
    }
}

fn error_kind(error: &rusqlite::Error) -> &'static str {
    match error {
        rusqlite::Error::SqliteFailure(failure, _) => match failure.code {
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => "busy",
            rusqlite::ErrorCode::DiskFull => "disk_full",
            rusqlite::ErrorCode::CannotOpen => "cannot_open",
            rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt => "corrupt",
            _ => "sqlite",
        },
        _ => "other",
    }
}

fn open_connection(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let connection = Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_millis(500))?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS usage_requests (
           id INTEGER PRIMARY KEY,
           at_ms INTEGER NOT NULL,
           provider TEXT NOT NULL,
           model TEXT NOT NULL,
           kind TEXT NOT NULL,
           thread_id TEXT,
           run_id TEXT,
           input_tokens INTEGER,
           cached_input_tokens INTEGER,
           output_tokens INTEGER,
           reasoning_tokens INTEGER
         );
         CREATE INDEX IF NOT EXISTS usage_requests_at ON usage_requests(at_ms);
         CREATE INDEX IF NOT EXISTS usage_requests_thread ON usage_requests(thread_id);
         CREATE TABLE IF NOT EXISTS usage_daily (
           day TEXT NOT NULL,
           provider TEXT NOT NULL,
           model TEXT NOT NULL,
           kind TEXT NOT NULL,
           thread_id TEXT NOT NULL DEFAULT '',
           requests INTEGER NOT NULL,
           unreported INTEGER NOT NULL,
           input_tokens INTEGER NOT NULL,
           cached_input_tokens INTEGER NOT NULL,
           output_tokens INTEGER NOT NULL,
           reasoning_tokens INTEGER NOT NULL,
           PRIMARY KEY (day, provider, model, kind, thread_id)
         );
         CREATE INDEX IF NOT EXISTS usage_daily_thread ON usage_daily(thread_id);",
    )?;
    Ok(connection)
}

fn write_rows(
    receiver: Receiver<UsageRow>,
    connection: Arc<Mutex<Connection>>,
    log: Arc<DiagnosticLog>,
) {
    let report = |stage: &str, error: &rusqlite::Error| {
        let _ = log.append(
            "usage",
            "write_failed",
            "warn",
            json!({"stage": stage, "error_kind": error_kind(error)}),
        );
    };
    let rollup_now = |connection: &Connection| {
        if let Err(error) = rollup(connection, Utc::now(), &chrono::Local) {
            report("rollup", &error);
        }
    };
    rollup_now(&connection.lock().unwrap_or_else(|e| e.into_inner()));
    let mut inserted = 0u64;
    while let Ok(row) = receiver.recv() {
        let connection = connection.lock().unwrap_or_else(|e| e.into_inner());
        match insert(&connection, &row) {
            Ok(()) => inserted += 1,
            Err(error) => report("insert", &error),
        }
        if inserted > 0 && inserted.is_multiple_of(ROLLUP_EVERY) {
            rollup_now(&connection);
        }
    }
}

pub fn insert(connection: &Connection, row: &UsageRow) -> rusqlite::Result<()> {
    let count = |value: Option<u64>| value.map(|value| value.min(i64::MAX as u64) as i64);
    connection.execute(
        "INSERT INTO usage_requests(at_ms,provider,model,kind,thread_id,run_id,input_tokens,cached_input_tokens,output_tokens,reasoning_tokens) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            row.at_ms,
            row.provider,
            row.model,
            kind_name(row.kind),
            row.thread_id,
            row.run_id,
            count(row.input_tokens),
            count(row.cached_input_tokens),
            count(row.output_tokens),
            count(row.reasoning_tokens),
        ],
    )?;
    Ok(())
}

fn read_rows(connection: &Connection, since_ms: i64) -> rusqlite::Result<Vec<UsageRow>> {
    let mut statement = connection.prepare(
        "SELECT at_ms,provider,model,kind,thread_id,run_id,input_tokens,cached_input_tokens,output_tokens,reasoning_tokens FROM usage_requests WHERE at_ms>=?1 ORDER BY at_ms",
    )?;
    let count = |value: Option<i64>| value.map(|value| value.max(0) as u64);
    let rows = statement.query_map([since_ms], |row| {
        Ok(UsageRow {
            at_ms: row.get(0)?,
            provider: row.get(1)?,
            model: row.get(2)?,
            kind: kind_from(&row.get::<_, String>(3)?),
            thread_id: row.get(4)?,
            run_id: row.get(5)?,
            input_tokens: count(row.get(6)?),
            cached_input_tokens: count(row.get(7)?),
            output_tokens: count(row.get(8)?),
            reasoning_tokens: count(row.get(9)?),
        })
    })?;
    rows.collect()
}

/// Moves raw rows older than [`RAW_RETENTION_DAYS`] into daily totals (local days in
/// `zone`) and removes daily totals older than [`DAILY_RETENTION_DAYS`], atomically.
pub fn rollup<Tz: TimeZone>(
    connection: &Connection,
    now: DateTime<Utc>,
    zone: &Tz,
) -> rusqlite::Result<usize> {
    let cutoff = (now - Duration::days(RAW_RETENTION_DAYS)).timestamp_millis();
    let old = {
        let mut statement = connection.prepare(
            "SELECT at_ms,provider,model,kind,thread_id,run_id,input_tokens,cached_input_tokens,output_tokens,reasoning_tokens FROM usage_requests WHERE at_ms<?1",
        )?;
        let count = |value: Option<i64>| value.map(|value| value.max(0) as u64);
        let rows = statement.query_map([cutoff], |row| {
            Ok(UsageRow {
                at_ms: row.get(0)?,
                provider: row.get(1)?,
                model: row.get(2)?,
                kind: kind_from(&row.get::<_, String>(3)?),
                thread_id: row.get(4)?,
                run_id: row.get(5)?,
                input_tokens: count(row.get(6)?),
                cached_input_tokens: count(row.get(7)?),
                output_tokens: count(row.get(8)?),
                reasoning_tokens: count(row.get(9)?),
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut days: BTreeMap<(String, String, String, &'static str, String), Totals> =
        BTreeMap::new();
    for row in &old {
        let day = local_day(row.at_ms, zone).to_string();
        days.entry((
            day,
            row.provider.clone(),
            row.model.clone(),
            kind_name(row.kind),
            row.thread_id.clone().unwrap_or_default(),
        ))
        .or_default()
        .add(row);
    }
    let oldest_day =
        (now.with_timezone(zone).date_naive() - Duration::days(DAILY_RETENTION_DAYS)).to_string();
    let transaction = connection.unchecked_transaction()?;
    for ((day, provider, model, kind, thread), totals) in &days {
        transaction.execute(
            "INSERT INTO usage_daily(day,provider,model,kind,thread_id,requests,unreported,input_tokens,cached_input_tokens,output_tokens,reasoning_tokens) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
             ON CONFLICT(day,provider,model,kind,thread_id) DO UPDATE SET requests=requests+excluded.requests,unreported=unreported+excluded.unreported,input_tokens=input_tokens+excluded.input_tokens,cached_input_tokens=cached_input_tokens+excluded.cached_input_tokens,output_tokens=output_tokens+excluded.output_tokens,reasoning_tokens=reasoning_tokens+excluded.reasoning_tokens",
            params![
                day,
                provider,
                model,
                kind,
                thread,
                totals.requests as i64,
                totals.unreported_requests as i64,
                totals.input_tokens as i64,
                totals.cached_input_tokens as i64,
                totals.output_tokens as i64,
                totals.reasoning_tokens as i64,
            ],
        )?;
    }
    transaction.execute("DELETE FROM usage_requests WHERE at_ms<?1", [cutoff])?;
    transaction.execute("DELETE FROM usage_daily WHERE day<?1", [oldest_day])?;
    transaction.commit()?;
    Ok(old.len())
}

/// Token sums for a set of requests. `input_tokens` includes the cached share;
/// `output_tokens` includes reasoning, as providers report them.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Totals {
    pub requests: u64,
    /// Requests whose provider reported no token counts.
    pub unreported_requests: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    /// Estimated list-price dollars of the priced requests; `None` when none was priced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_cost_usd: Option<f64>,
    /// Requests whose model has no published price (not in the estimate).
    pub unpriced_requests: u64,
}

impl Totals {
    fn add(&mut self, row: &UsageRow) {
        self.requests += 1;
        if row.input_tokens.is_none() && row.output_tokens.is_none() {
            self.unreported_requests += 1;
        }
        self.input_tokens += row.input_tokens.unwrap_or(0);
        self.cached_input_tokens += row.cached_input_tokens.unwrap_or(0);
        self.output_tokens += row.output_tokens.unwrap_or(0);
        self.reasoning_tokens += row.reasoning_tokens.unwrap_or(0);
    }

    fn add_priced(&mut self, row: &UsageRow, price: Option<&ModelPrice>) {
        self.add(row);
        match price {
            Some(price) => {
                let cost = price.request_cost(
                    row.input_tokens.unwrap_or(0),
                    row.cached_input_tokens.unwrap_or(0),
                    row.output_tokens.unwrap_or(0),
                );
                *self.estimated_cost_usd.get_or_insert(0.0) += cost;
            }
            None => self.unpriced_requests += 1,
        }
    }
}

/// How a provider bills, so cost or allowance can be shown once a provider supplies
/// verified numbers. Nothing here is shown as an allowance today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingKind {
    /// A flat plan with usage limits per time window (OpenCode Go).
    PlanWindows,
    PayPerToken,
    Free,
    Local,
}

/// A usage limit of a plan window, in dollars of list-price usage.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanWindow {
    pub window: String,
    pub limit_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProviderBilling {
    pub provider: String,
    pub kind: BillingKind,
    /// Verified plan limits; empty until a provider publishes them.
    pub windows: Vec<PlanWindow>,
    /// Where estimated prices come from, when any model has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Windows {
    pub five_hours: Totals,
    pub today: Totals,
    pub week: Totals,
    pub month: Totals,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DayUsage {
    /// Local date, `YYYY-MM-DD`.
    pub day: String,
    /// Input + output tokens per model.
    pub models: BTreeMap<String, u64>,
    pub requests: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Breakdown {
    /// Provider, model or chat id; `None` for requests without a chat.
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Chat title, when the chat still exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The model's published price is zero.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub free: bool,
    pub totals: Totals,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UsageSummary {
    pub available: bool,
    /// Only requests this Themis made; provider dashboards may count more.
    pub scope: &'static str,
    pub generated_at_ms: i64,
    /// The week starts on Monday, local time.
    pub week_starts: &'static str,
    pub windows: Windows,
    pub days: Vec<DayUsage>,
    /// Breakdowns over the chart's 30 days, largest first.
    pub by_provider: Vec<Breakdown>,
    pub by_model: Vec<Breakdown>,
    pub by_chat: Vec<Breakdown>,
    pub billing: Vec<ProviderBilling>,
}

fn local_day<Tz: TimeZone>(at_ms: i64, zone: &Tz) -> NaiveDate {
    Utc.timestamp_millis_opt(at_ms)
        .single()
        .unwrap_or_default()
        .with_timezone(zone)
        .date_naive()
}

/// The first instant of `date` in `zone` (the earliest one when midnight repeats; the
/// first hour that exists when midnight is skipped).
pub fn local_start<Tz: TimeZone>(zone: &Tz, date: NaiveDate) -> DateTime<Utc> {
    let midnight = date.and_hms_opt(0, 0, 0).unwrap_or_default();
    (0..=3)
        .find_map(|hours| {
            zone.from_local_datetime(&(midnight + Duration::hours(hours)))
                .earliest()
        })
        .map(|start| start.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&midnight))
}

/// Window starts for `now` in `zone`: 5 hours ago, today, this week (Monday), this
/// month, and the chart's first day.
pub struct WindowStarts {
    pub five_hours: i64,
    pub today: i64,
    pub week: i64,
    pub month: i64,
    pub chart: i64,
    pub first_chart_day: NaiveDate,
}

pub fn window_starts<Tz: TimeZone>(now: DateTime<Utc>, zone: &Tz) -> WindowStarts {
    let today = now.with_timezone(zone).date_naive();
    let monday = today - Duration::days(i64::from(today.weekday().num_days_from_monday()));
    let first = today.with_day(1).unwrap_or(today);
    let first_chart_day = today - Duration::days(CHART_DAYS as i64 - 1);
    WindowStarts {
        five_hours: (now - Duration::hours(5)).timestamp_millis(),
        today: local_start(zone, today).timestamp_millis(),
        week: local_start(zone, monday).timestamp_millis(),
        month: local_start(zone, first).timestamp_millis(),
        chart: local_start(zone, first_chart_day).timestamp_millis(),
        first_chart_day,
    }
}

impl WindowStarts {
    /// Earliest instant any window reads.
    pub fn earliest(&self) -> i64 {
        self.five_hours
            .min(self.today)
            .min(self.week)
            .min(self.month)
            .min(self.chart)
    }
}

/// Sums `rows` into the windows, the daily chart and the breakdowns. `prices` holds
/// published list prices by (provider, model); `titles` names chats that still exist.
pub fn summarize<Tz: TimeZone>(
    rows: &[UsageRow],
    now: DateTime<Utc>,
    zone: &Tz,
    prices: &HashMap<(String, String), ModelPrice>,
    titles: &HashMap<String, String>,
) -> UsageSummary {
    let starts = window_starts(now, zone);
    let now_ms = now.timestamp_millis();
    let mut windows = Windows {
        five_hours: Totals::default(),
        today: Totals::default(),
        week: Totals::default(),
        month: Totals::default(),
    };
    let mut days: Vec<DayUsage> = (0..CHART_DAYS)
        .map(|offset| DayUsage {
            day: (starts.first_chart_day + Duration::days(offset as i64)).to_string(),
            models: BTreeMap::new(),
            requests: 0,
        })
        .collect();
    let mut providers: HashMap<String, Totals> = HashMap::new();
    let mut models: HashMap<(String, String), Totals> = HashMap::new();
    let mut chats: HashMap<Option<String>, Totals> = HashMap::new();
    for row in rows.iter().filter(|row| row.at_ms <= now_ms) {
        let price = prices.get(&(row.provider.clone(), row.model.clone()));
        for (start, totals) in [
            (starts.five_hours, &mut windows.five_hours),
            (starts.today, &mut windows.today),
            (starts.week, &mut windows.week),
            (starts.month, &mut windows.month),
        ] {
            if row.at_ms >= start {
                totals.add_priced(row, price);
            }
        }
        if row.at_ms < starts.chart {
            continue;
        }
        let day = (local_day(row.at_ms, zone) - starts.first_chart_day).num_days();
        if let Some(day) = usize::try_from(day).ok().and_then(|day| days.get_mut(day)) {
            *day.models.entry(row.model.clone()).or_default() +=
                row.input_tokens.unwrap_or(0) + row.output_tokens.unwrap_or(0);
            day.requests += 1;
        }
        providers
            .entry(row.provider.clone())
            .or_default()
            .add_priced(row, price);
        models
            .entry((row.provider.clone(), row.model.clone()))
            .or_default()
            .add_priced(row, price);
        // Requests of deleted chats are summed under no chat id.
        let chat = row
            .thread_id
            .clone()
            .filter(|thread| titles.contains_key(thread));
        chats.entry(chat).or_default().add_priced(row, price);
    }
    let size = |totals: &Totals| totals.input_tokens + totals.output_tokens;
    let sorted = |mut list: Vec<Breakdown>| {
        list.sort_by(|a, b| {
            size(&b.totals)
                .cmp(&size(&a.totals))
                .then_with(|| a.key.cmp(&b.key))
        });
        list
    };
    let by_provider = sorted(
        providers
            .into_iter()
            .map(|(provider, totals)| Breakdown {
                key: Some(provider),
                provider: None,
                title: None,
                free: false,
                totals,
            })
            .collect(),
    );
    let by_model = sorted(
        models
            .into_iter()
            .map(|((provider, model), totals)| Breakdown {
                free: prices
                    .get(&(provider.clone(), model.clone()))
                    .is_some_and(ModelPrice::is_free),
                key: Some(model),
                provider: Some(provider),
                title: None,
                totals,
            })
            .collect(),
    );
    let mut by_chat = sorted(
        chats
            .into_iter()
            .map(|(chat, totals)| Breakdown {
                title: chat.as_ref().and_then(|chat| titles.get(chat).cloned()),
                key: chat,
                provider: None,
                free: false,
                totals,
            })
            .collect(),
    );
    // Named chats first; the rest (deleted chats, requests without one) last.
    by_chat.sort_by_key(|chat| chat.key.is_none());
    by_chat.truncate(TOP_CHATS + 1);
    let price_source = (!prices.is_empty()).then(|| "models.dev".to_owned());
    UsageSummary {
        available: true,
        scope: "themis_only",
        generated_at_ms: now_ms,
        week_starts: "monday",
        windows,
        days,
        billing: by_provider
            .iter()
            .filter_map(|provider| provider.key.clone())
            .map(|provider| ProviderBilling {
                kind: if provider == themis_core::providers::GO_PROVIDER_ID {
                    BillingKind::PlanWindows
                } else {
                    BillingKind::PayPerToken
                },
                provider,
                windows: Vec::new(),
                price_source: price_source.clone(),
            })
            .collect(),
        by_provider,
        by_model,
        by_chat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn row(
        at: DateTime<Utc>,
        model: &str,
        thread: Option<&str>,
        input: u64,
        output: u64,
    ) -> UsageRow {
        UsageRow {
            at_ms: at.timestamp_millis(),
            provider: "opencode-go".into(),
            model: model.into(),
            kind: UsageKind::Answer,
            thread_id: thread.map(String::from),
            run_id: Some("run".into()),
            input_tokens: Some(input),
            cached_input_tokens: Some(input / 2),
            output_tokens: Some(output),
            reasoning_tokens: Some(output / 4),
        }
    }

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn windows_follow_the_local_day_week_and_month() {
        // Wednesday 2026-10-07 01:30 in UTC+9 is Tuesday 16:30 UTC.
        let zone = FixedOffset::east_opt(9 * 3600).unwrap();
        let now = utc("2026-10-06T16:30:00Z");
        let rows = [
            row(utc("2026-10-06T15:10:00Z"), "a", None, 10, 1), // local Wed 00:10: today
            row(utc("2026-10-06T14:50:00Z"), "a", None, 20, 2), // local Tue 23:50: week, not today
            row(utc("2026-10-04T15:30:00Z"), "a", None, 40, 4), // local Mon 00:30: week
            row(utc("2026-10-04T14:30:00Z"), "a", None, 80, 8), // local Sun 23:30: month only
            row(utc("2026-09-30T14:59:00Z"), "a", None, 160, 16), // local Sep 30 23:59: neither
            row(utc("2026-10-06T17:00:00Z"), "a", None, 999, 9), // the future: ignored
        ];
        let summary = summarize(&rows, now, &zone, &HashMap::new(), &HashMap::new());
        assert_eq!(summary.windows.five_hours.input_tokens, 30);
        assert_eq!(summary.windows.today.input_tokens, 10);
        assert_eq!(summary.windows.week.input_tokens, 70);
        assert_eq!(summary.windows.month.input_tokens, 150);
        assert_eq!(summary.windows.month.requests, 4);
        assert_eq!(summary.windows.today.cached_input_tokens, 5);
        assert_eq!(summary.windows.today.reasoning_tokens, 0);
        assert_eq!(summary.days.len(), CHART_DAYS);
        assert_eq!(summary.days.last().unwrap().day, "2026-10-07");
        assert_eq!(summary.days.last().unwrap().models["a"], 11);
        assert_eq!(summary.days[CHART_DAYS - 2].models["a"], 22);
        assert_eq!(summary.days[CHART_DAYS - 8].day, "2026-09-30");
        assert_eq!(summary.days[CHART_DAYS - 8].models["a"], 176);
    }

    #[test]
    fn a_week_that_starts_in_the_previous_month_and_a_dst_change() {
        let zone = chrono_tz::Europe::Berlin;
        // Sunday 2026-11-01 10:00 local (UTC+1 after the 25 October change).
        let now = utc("2026-11-01T09:00:00Z");
        let starts = window_starts(now, &zone);
        // Monday 26 October 00:00 local is UTC+1: 25 October 23:00 UTC.
        assert_eq!(starts.week, utc("2026-10-25T23:00:00Z").timestamp_millis());
        assert_eq!(starts.month, utc("2026-10-31T23:00:00Z").timestamp_millis());
        assert_eq!(starts.today, starts.month);
        // The chart's first day, 3 October, is still summer time (UTC+2).
        assert_eq!(starts.chart, utc("2026-10-02T22:00:00Z").timestamp_millis());
        let rows = [row(utc("2026-10-25T22:30:00Z"), "a", None, 5, 0)];
        let summary = summarize(&rows, now, &zone, &HashMap::new(), &HashMap::new());
        assert_eq!(summary.windows.week.requests, 0, "Sunday 23:30 local");
        assert_eq!(summary.windows.month.requests, 0);
        assert_eq!(summary.days[CHART_DAYS - 8].day, "2026-10-25");
        assert_eq!(summary.days[CHART_DAYS - 8].requests, 1);
    }

    #[test]
    fn a_skipped_midnight_starts_the_day_at_its_first_hour() {
        // Santiago skips 00:00–01:00 on 6 September 2026.
        let zone = chrono_tz::America::Santiago;
        let start = local_start(&zone, NaiveDate::from_ymd_opt(2026, 9, 6).unwrap());
        assert_eq!(start, utc("2026-09-06T04:00:00Z"));
    }

    #[test]
    fn breakdowns_price_models_and_hide_deleted_chats() {
        let zone = Utc;
        let now = utc("2026-10-20T12:00:00Z");
        let rows = [
            row(
                utc("2026-10-20T10:00:00Z"),
                "paid",
                Some("chat-1"),
                1_000_000,
                100_000,
            ),
            row(utc("2026-10-20T11:00:00Z"), "free", Some("gone"), 500, 50),
            row(
                utc("2026-10-19T11:00:00Z"),
                "unknown",
                Some("chat-2"),
                10,
                1,
            ),
            UsageRow {
                input_tokens: None,
                cached_input_tokens: None,
                output_tokens: None,
                reasoning_tokens: None,
                ..row(utc("2026-10-20T11:30:00Z"), "paid", Some("chat-1"), 0, 0)
            },
        ];
        let prices = HashMap::from([
            (
                ("opencode-go".into(), "paid".into()),
                ModelPrice {
                    input: 1.0,
                    output: 2.0,
                    cache_read: Some(0.1),
                    tiers: vec![],
                },
            ),
            (("opencode-go".into(), "free".into()), ModelPrice::default()),
        ]);
        let titles = HashMap::from([
            ("chat-1".to_owned(), "First".to_owned()),
            ("chat-2".to_owned(), "Second".to_owned()),
        ]);
        let summary = summarize(&rows, now, &zone, &prices, &titles);
        let today = &summary.windows.today;
        assert_eq!(today.requests, 3);
        assert_eq!(today.unreported_requests, 1);
        // 0.5M uncached × $1 + 0.5M cached × $0.1 + 0.1M × $2 = $0.75; free adds $0.
        assert!((today.estimated_cost_usd.unwrap() - 0.75).abs() < 1e-9);
        assert_eq!(today.unpriced_requests, 0);
        assert_eq!(summary.windows.month.unpriced_requests, 1);
        assert_eq!(summary.by_model[0].key.as_deref(), Some("paid"));
        assert!(summary
            .by_model
            .iter()
            .any(|m| m.key.as_deref() == Some("free") && m.free));
        assert_eq!(summary.by_chat[0].title.as_deref(), Some("First"));
        assert_eq!(summary.by_chat[1].title.as_deref(), Some("Second"));
        assert_eq!(summary.by_chat[2].key, None, "deleted chats last");
        assert_eq!(summary.billing[0].kind, BillingKind::PlanWindows);
        assert!(summary.billing[0].windows.is_empty());
    }

    #[test]
    fn rows_are_stored_and_rolled_into_daily_totals_without_losing_chat_totals() {
        let directory = tempfile::tempdir().unwrap();
        let connection = open_connection(&directory.path().join("usage.sqlite3")).unwrap();
        let now = utc("2026-10-20T12:00:00Z");
        let old = now - Duration::days(RAW_RETENTION_DAYS + 1);
        for at in [old, old + Duration::minutes(5), now - Duration::hours(1)] {
            insert(&connection, &row(at, "m", Some("chat"), 100, 10)).unwrap();
        }
        insert(
            &connection,
            &row(
                now - Duration::days(DAILY_RETENTION_DAYS + 40),
                "m",
                None,
                1,
                1,
            ),
        )
        .unwrap();
        assert_eq!(rollup(&connection, now, &Utc).unwrap(), 3);
        assert_eq!(rollup(&connection, now, &Utc).unwrap(), 0, "idempotent");
        assert_eq!(read_rows(&connection, 0).unwrap().len(), 1);
        let (days, requests): (i64, i64) = connection
            .query_row(
                "SELECT COUNT(*),SUM(requests) FROM usage_daily",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (days, requests),
            (1, 2),
            "one day kept; the too-old one removed"
        );
        let store = UsageStore {
            connection: Some(Arc::new(Mutex::new(connection))),
            sender: Mutex::new(None),
            dropped: Arc::default(),
            diagnostics: Arc::new(DiagnosticLog::new(directory.path().join("logs"))),
        };
        let chat = store.chat_totals("chat").unwrap();
        assert_eq!(
            (chat.requests, chat.input_tokens, chat.output_tokens),
            (3, 300, 30)
        );
        assert_eq!(store.chat_totals("other").unwrap().requests, 0);
    }

    #[test]
    fn the_writer_stores_queued_rows_and_ids_are_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let log = Arc::new(DiagnosticLog::new(directory.path().join("logs")));
        let store = UsageStore::open(&directory.path().join("usage.sqlite3"), log);
        assert!(store.available());
        let long = "x".repeat(500);
        store.record(UsageRow::from_record(
            UsageRecord {
                provider: "opencode-go".into(),
                model: long.clone(),
                kind: UsageKind::GoalCheck,
                input_tokens: Some(7),
                cached_input_tokens: None,
                output_tokens: Some(3),
                reasoning_tokens: None,
            },
            Utc::now().timestamp_millis(),
            Some(&long),
            Some("run-1"),
        ));
        let mut rows = Vec::new();
        for _ in 0..200 {
            rows = store.rows_since(0).unwrap();
            if !rows.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].model.len(), MAX_ID_CHARS);
        assert_eq!(rows[0].thread_id.as_ref().unwrap().len(), MAX_ID_CHARS);
        assert_eq!(rows[0].kind, UsageKind::GoalCheck);
        assert_eq!(rows[0].output_tokens, Some(3));
    }
}
