//! Background shell tasks: long-running programs (dev servers, watchers) that
//! outlive the tool call that started them.
//!
//! A task is one program in its own process group, owned by the chat that
//! started it. Output (stdout and stderr, interleaved) goes to a bounded
//! rolling log ([`LogRing`]) captured on helper threads so a chatty child never
//! blocks. One supervisor thread per task reaps the child, enforces the
//! per-task deadline, honours stop requests and, when the main process exits,
//! stops whatever is left of its process group so nothing is orphaned.
//!
//! Lifetime: tasks are killed (whole group) when the chat is removed
//! ([`BackgroundRegistry::remove_thread`]), when the app server shuts down
//! ([`BackgroundRegistry::shutdown`]) and at their deadline. Records are
//! persisted beside the app data; a record still marked running when a
//! registry loads (the previous server died without cleanup) becomes
//! [`TaskState::Lost`], never running.

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use autoagents_derive::{tool, ToolInput};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{
    async_trait, kill_process_tree, parse_tool_args, tool_error, ToolCallError, ToolRuntime, ToolT,
};

mod log_ring;
mod process;
mod tool;

pub use log_ring::{LogChunk, LogRing};
use process::{pump, request_stop, wait_finished};
pub use tool::{only_inspects, BackgroundArgs, BackgroundScope, BackgroundTool};

/// Rolling log capacity per task, in bytes.
pub const MAX_LOG_BYTES: usize = 64 * 1024;
/// Largest log page a single read returns, in bytes.
pub const MAX_LOG_PAGE_BYTES: usize = 16 * 1024;
/// Default log page size, in bytes.
pub const DEFAULT_LOG_PAGE_BYTES: usize = 8 * 1024;
/// Running background tasks allowed per chat.
pub const MAX_TASKS_PER_THREAD: usize = 4;
/// Running background tasks allowed across the whole app server.
pub const MAX_TASKS_TOTAL: usize = 16;
/// Finished task records kept per chat (oldest are dropped first).
pub const MAX_FINISHED_PER_THREAD: usize = 20;
/// Default per-task deadline when the caller gives none.
pub const DEFAULT_TASK_TIMEOUT_SECS: u64 = 2 * 60 * 60;
/// Longest per-task deadline a caller may request.
pub const MAX_TASK_TIMEOUT_SECS: u64 = 24 * 60 * 60;
/// Lines of recent output shown by `status`.
pub const STATUS_TAIL_LINES: usize = 20;
/// Longest recent-output line, in characters.
const MAX_TAIL_LINE_CHARS: usize = 240;
/// Longest command summary, in characters.
const MAX_SUMMARY_CHARS: usize = 160;
/// How long stop/removal waits for a task to be reaped before reporting.
pub const STOP_WAIT: Duration = Duration::from_secs(5);
const READER_GRACE: Duration = Duration::from_millis(500);
const SUPERVISOR_TICK: Duration = Duration::from_millis(25);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

// ---------------------------------------------------------------------------
// Records and views
// ---------------------------------------------------------------------------

/// Lifecycle of a background task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// The program is running.
    Running,
    /// The main process exited on its own (see `exit_code`).
    Exited,
    /// Stopped on request, chat removal or server shutdown.
    Stopped,
    /// Killed at its deadline.
    TimedOut,
    /// The server that started it died; the process may still exist but is
    /// not tracked.
    Lost,
}

/// Durable facts about one task (no output).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: String,
    pub thread_id: String,
    /// The run that started it, when known (groups finished work by request).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub command: Vec<String>,
    pub cwd: String,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub state: TaskState,
    pub exit_code: Option<i32>,
    pub pid: u32,
    pub timeout_secs: u64,
}

/// A task as reported to the model and the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskView {
    #[serde(flatten)]
    pub record: TaskRecord,
    /// One-line, length-bounded command.
    pub summary: String,
    /// Bytes of output produced so far.
    pub log_bytes: u64,
    /// Most recent output lines.
    pub tail: Vec<String>,
    /// `tail` is the entire output, so a full log would repeat it.
    pub tail_complete: bool,
}

/// Command line shortened to one bounded line.
pub fn summarize_command(argv: &[String]) -> String {
    let line = argv.join(" ").replace(['\n', '\r'], " ");
    if line.chars().count() <= MAX_SUMMARY_CHARS {
        return line;
    }
    let mut short: String = line.chars().take(MAX_SUMMARY_CHARS - 1).collect();
    short.push('…');
    short
}

#[derive(Serialize, Deserialize)]
struct Store {
    version: u32,
    tasks: Vec<TaskRecord>,
}

const STOP_NONE: u8 = 0;
const STOP_REQUESTED: u8 = 1;

struct Task {
    record: Mutex<TaskRecord>,
    finished: Condvar,
    log: Mutex<LogRing>,
    stop: AtomicU8,
    /// Set once the final state is recorded, persisted and announced.
    settled: AtomicBool,
}

impl Task {
    fn record(&self) -> TaskRecord {
        lock(&self.record).clone()
    }
}

/// Callback invoked with the chat id after any task of that chat changes.
pub type Notifier = Arc<dyn Fn(&str) + Send + Sync>;

/// Parameters for [`BackgroundRegistry::start`].
pub struct StartSpec {
    pub thread_id: String,
    /// The run that starts it.
    pub run_id: Option<String>,
    pub program: String,
    pub args: Vec<String>,
    pub workdir: PathBuf,
    /// Per-task deadline in seconds; 0 selects the default.
    pub timeout_secs: u64,
    /// Sandbox the task runs in (the run's captured plan); `None` runs it as is.
    pub sandbox: Option<Arc<crate::sandbox::SandboxPlan>>,
    /// The `bin` folders of the run's plugins.
    pub plugin_bins: Vec<PathBuf>,
}

/// Tracks background tasks for every chat of one app server.
pub struct BackgroundRegistry {
    dir: Option<PathBuf>,
    max_per_thread: usize,
    max_total: usize,
    tasks: Mutex<Vec<Arc<Task>>>,
    notifier: Mutex<Option<Notifier>>,
    persist_lock: Mutex<()>,
    counter: AtomicU64,
}

impl BackgroundRegistry {
    /// Registry without persistence (tests, embedded callers).
    pub fn in_memory() -> Arc<Self> {
        Self::open(None)
    }

    /// Registry persisting records and finished logs under `dir`.
    pub fn open(dir: Option<PathBuf>) -> Arc<Self> {
        Self::open_with_limits(dir, MAX_TASKS_PER_THREAD, MAX_TASKS_TOTAL)
    }

    /// [`open`](Self::open) with explicit concurrency limits.
    pub fn open_with_limits(
        dir: Option<PathBuf>,
        max_per_thread: usize,
        max_total: usize,
    ) -> Arc<Self> {
        let registry = Arc::new(Self {
            dir,
            max_per_thread,
            max_total,
            tasks: Mutex::new(Vec::new()),
            notifier: Mutex::new(None),
            persist_lock: Mutex::new(()),
            counter: AtomicU64::new(0),
        });
        registry.load();
        registry
    }

    /// Registers the change callback (replaces any earlier one).
    pub fn set_notifier(&self, notifier: Notifier) {
        *lock(&self.notifier) = Some(notifier);
    }

    fn notify(&self, thread_id: &str) {
        let notifier = lock(&self.notifier).clone();
        if let Some(notifier) = notifier {
            notifier(thread_id);
        }
    }

    fn store_path(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|dir| dir.join("tasks.json"))
    }

    fn log_path(&self, id: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|dir| dir.join(format!("{id}.log")))
    }

    fn load(&self) {
        let Some(path) = self.store_path() else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        let Ok(store) = serde_json::from_str::<Store>(&text) else {
            return;
        };
        let mut changed = false;
        let mut tasks = lock(&self.tasks);
        for mut record in store.tasks {
            if record.state == TaskState::Running {
                record.state = TaskState::Lost;
                record.ended_at_ms.get_or_insert_with(now_ms);
                changed = true;
            }
            let log = self
                .log_path(&record.id)
                .and_then(|path| std::fs::read(path).ok())
                .map_or_else(
                    || LogRing::new(MAX_LOG_BYTES),
                    |bytes| LogRing::from_bytes(MAX_LOG_BYTES, &bytes),
                );
            tasks.push(Arc::new(Task {
                record: Mutex::new(record),
                finished: Condvar::new(),
                log: Mutex::new(log),
                stop: AtomicU8::new(STOP_NONE),
                settled: AtomicBool::new(true),
            }));
        }
        drop(tasks);
        if changed {
            self.persist();
        }
    }

    fn persist(&self) {
        let (Some(dir), Some(path)) = (&self.dir, self.store_path()) else {
            return;
        };
        let _guard = lock(&self.persist_lock);
        let tasks: Vec<TaskRecord> = lock(&self.tasks).iter().map(|task| task.record()).collect();
        let Ok(text) = serde_json::to_vec(&Store { version: 1, tasks }) else {
            return;
        };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let temporary = path.with_extension("json.tmp");
        if std::fs::write(&temporary, text).is_ok() {
            let _ = std::fs::rename(&temporary, &path);
        }
    }

    fn new_id(&self) -> String {
        loop {
            let mut hasher = Sha256::new();
            hasher.update(now_ms().to_le_bytes());
            hasher.update(self.counter.fetch_add(1, Ordering::Relaxed).to_le_bytes());
            hasher.update(std::process::id().to_le_bytes());
            let digest = hasher.finalize();
            let id = format!("bg_{:02x}{:02x}{:02x}", digest[0], digest[1], digest[2]);
            if !lock(&self.tasks)
                .iter()
                .any(|task| lock(&task.record).id == id)
            {
                return id;
            }
        }
    }

    /// Starts `spec` and returns immediately.
    ///
    /// The child gets the same scrubbed environment, no stdin and its own
    /// process group as a blocking `shell` call. Fails when the chat or the
    /// server already runs its maximum number of background tasks.
    pub fn start(self: &Arc<Self>, spec: StartSpec) -> Result<TaskView, String> {
        let timeout_secs = match spec.timeout_secs {
            0 => DEFAULT_TASK_TIMEOUT_SECS,
            seconds => seconds.min(MAX_TASK_TIMEOUT_SECS),
        };
        let id = self.new_id();
        let mut tasks = lock(&self.tasks);
        let running: Vec<TaskRecord> = tasks
            .iter()
            .map(|task| task.record())
            .filter(|record| record.state == TaskState::Running)
            .collect();
        let in_thread = running
            .iter()
            .filter(|record| record.thread_id == spec.thread_id)
            .count();
        if in_thread >= self.max_per_thread {
            return Err(format!(
                "this chat already has {in_thread} running background tasks (limit {}); stop one with background_task first",
                self.max_per_thread
            ));
        }
        if running.len() >= self.max_total {
            return Err(format!(
                "the app already runs {} background tasks (limit {}); stop one first",
                running.len(),
                self.max_total
            ));
        }
        if let Some(refusal) = spec.sandbox.as_ref().and_then(|plan| plan.refusal()) {
            return Err(refusal);
        }
        let prepared = super::prepare_command(
            &spec.program,
            &spec.args,
            spec.sandbox.as_deref(),
            &spec.plugin_bins,
        )
        .map_err(|error| super::spawn_failure(&spec.program, &spec.args, &error))?;
        let mut command = prepared.command();
        command
            .current_dir(&spec.workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .envs(super::scrubbed_env());
        if let Some(path) = super::child_path(&spec.plugin_bins) {
            command.env("PATH", path);
        }
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command
            .spawn()
            .map_err(|error| super::spawn_failure(&spec.program, &spec.args, &error))?;
        let argv: Vec<String> = std::iter::once(spec.program.clone())
            .chain(spec.args.iter().cloned())
            .collect();
        let task = Arc::new(Task {
            record: Mutex::new(TaskRecord {
                id,
                thread_id: spec.thread_id.clone(),
                run_id: spec.run_id.clone(),
                command: argv,
                cwd: spec.workdir.to_string_lossy().into_owned(),
                started_at_ms: now_ms(),
                ended_at_ms: None,
                state: TaskState::Running,
                exit_code: None,
                pid: child.id(),
                timeout_secs,
            }),
            finished: Condvar::new(),
            log: Mutex::new(LogRing::new(MAX_LOG_BYTES)),
            stop: AtomicU8::new(STOP_NONE),
            settled: AtomicBool::new(false),
        });
        tasks.push(Arc::clone(&task));
        drop(tasks);

        let mut readers = Vec::new();
        if let Some(pipe) = child.stdout.take() {
            readers.push(pump(pipe, Arc::clone(&task)));
        }
        if let Some(pipe) = child.stderr.take() {
            readers.push(pump(pipe, Arc::clone(&task)));
        }
        let registry = Arc::clone(self);
        let supervised = Arc::clone(&task);
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        std::thread::spawn(move || registry.supervise(&supervised, child, readers, deadline));
        self.persist();
        self.notify(&spec.thread_id);
        Ok(self.view(&task, STATUS_TAIL_LINES.min(3)))
    }

    fn find(&self, thread_id: &str, id: &str) -> Result<Arc<Task>, String> {
        lock(&self.tasks)
            .iter()
            .find(|task| {
                let record = lock(&task.record);
                record.id == id && record.thread_id == thread_id
            })
            .cloned()
            .ok_or_else(|| format!("unknown background task '{id}' in this chat"))
    }

    fn view(&self, task: &Task, tail_lines: usize) -> TaskView {
        let record = task.record();
        let (log_bytes, (tail, tail_complete)) = {
            let log = lock(&task.log);
            (log.total(), log.tail_lines_complete(tail_lines))
        };
        TaskView {
            summary: summarize_command(&record.command),
            record,
            log_bytes,
            tail,
            tail_complete,
        }
    }

    /// All tasks of one chat, oldest first.
    pub fn list(&self, thread_id: &str, tail_lines: usize) -> Vec<TaskView> {
        let tasks: Vec<Arc<Task>> = lock(&self.tasks).clone();
        let mut views: Vec<TaskView> = tasks
            .iter()
            .filter(|task| lock(&task.record).thread_id == thread_id)
            .map(|task| self.view(task, tail_lines))
            .collect();
        views.sort_by_key(|view| view.record.started_at_ms);
        views
    }

    /// One task with its most recent output.
    pub fn status(&self, thread_id: &str, id: &str) -> Result<TaskView, String> {
        Ok(self.view(self.find(thread_id, id)?.as_ref(), STATUS_TAIL_LINES))
    }

    /// A page of the rolling log. `offset: None` returns the newest output.
    pub fn log(
        &self,
        thread_id: &str,
        id: &str,
        offset: Option<u64>,
        max_bytes: usize,
    ) -> Result<(TaskRecord, LogChunk), String> {
        let task = self.find(thread_id, id)?;
        let max_bytes = max_bytes.clamp(1, MAX_LOG_PAGE_BYTES);
        let record = task.record();
        let settled = record.state != TaskState::Running;
        let log = lock(&task.log);
        let chunk = match offset {
            Some(offset) => log.read_with(offset, max_bytes, settled),
            None if settled => log.tail_settled(max_bytes),
            None => log.tail(max_bytes),
        };
        drop(log);
        Ok((record, chunk))
    }

    /// Stops one task (whole process group) and waits for it to be reaped.
    /// Blocking: call from a blocking context.
    pub fn stop(&self, thread_id: &str, id: &str, wait: Duration) -> Result<TaskView, String> {
        let task = self.find(thread_id, id)?;
        request_stop(&task);
        wait_finished(&task, wait);
        Ok(self.view(&task, STATUS_TAIL_LINES))
    }

    /// Stops every running task of a chat and forgets its records and logs.
    /// Blocking: call from a blocking context.
    pub fn remove_thread(&self, thread_id: &str, wait: Duration) {
        let owned: Vec<Arc<Task>> = lock(&self.tasks)
            .iter()
            .filter(|task| lock(&task.record).thread_id == thread_id)
            .cloned()
            .collect();
        if owned.is_empty() {
            return;
        }
        owned.iter().for_each(|task| request_stop(task));
        owned.iter().for_each(|task| wait_finished(task, wait));
        let ids: Vec<String> = owned.iter().map(|task| task.record().id).collect();
        lock(&self.tasks).retain(|task| !ids.contains(&lock(&task.record).id));
        for id in &ids {
            if let Some(path) = self.log_path(id) {
                let _ = std::fs::remove_file(path);
            }
        }
        self.persist();
        self.notify(thread_id);
    }

    /// Stops every running task (app server shutdown). Records are kept as
    /// stopped. Blocking: call from a blocking context.
    pub fn shutdown(&self, wait: Duration) {
        let tasks: Vec<Arc<Task>> = lock(&self.tasks).clone();
        tasks.iter().for_each(|task| request_stop(task));
        tasks.iter().for_each(|task| wait_finished(task, wait));
    }

    /// Running tasks across all chats.
    pub fn running_count(&self) -> usize {
        lock(&self.tasks)
            .iter()
            .filter(|task| lock(&task.record).state == TaskState::Running)
            .count()
    }
}

/// Directory used to persist background tasks beside the app data.
pub fn storage_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("background")
}

#[cfg(all(test, unix))]
mod own_tasks_tests {
    use super::*;

    /// A subagent's scope reaches only what it started: it can't see or stop the chat's others.
    #[tokio::test]
    async fn a_scope_limited_to_its_run_cannot_reach_other_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let registry = BackgroundRegistry::open(None);
        let start = |run: &str| {
            let registry = Arc::clone(&registry);
            let (run, workdir) = (run.to_owned(), dir.path().to_owned());
            tokio::task::spawn_blocking(move || {
                registry.start(StartSpec {
                    thread_id: "chat".into(),
                    run_id: Some(run),
                    program: "sleep".into(),
                    args: vec!["30".into()],
                    workdir,
                    timeout_secs: 60,
                    sandbox: None,
                    plugin_bins: Vec::new(),
                })
            })
        };
        let main = start("main").await.unwrap().unwrap().record.id;
        let own = start("child").await.unwrap().unwrap().record.id;
        let tool = BackgroundTool::new(Some(
            BackgroundScope::new(Arc::clone(&registry), "chat")
                .with_run("child")
                .own_tasks_only(),
        ));
        let listed = tool.execute(json!({"action": "list"})).await.unwrap();
        assert_eq!(listed["tasks"].as_array().unwrap().len(), 1, "{listed}");
        assert_eq!(listed["tasks"][0]["task_id"], own.as_str());
        for action in ["status", "log", "stop"] {
            let refused = tool
                .execute(json!({"action": action, "task_id": main}))
                .await;
            assert!(refused.is_err(), "{action} reached another task");
        }
        assert_eq!(
            registry.status("chat", &main).unwrap().record.state,
            TaskState::Running
        );
        let stopped = tool
            .execute(json!({"action": "stop", "task_id": own}))
            .await
            .unwrap();
        assert_eq!(stopped["state"], "stopped", "{stopped}");
        // Without the limit, as for the main agent, it can.
        let main_tool = BackgroundTool::new(Some(BackgroundScope::new(registry, "chat")));
        assert!(main_tool
            .execute(json!({"action": "stop", "task_id": main}))
            .await
            .is_ok());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn ring_keeps_only_the_newest_bytes_with_absolute_offsets() {
        let mut ring = LogRing::new(8);
        ring.push(b"abcd");
        ring.push(b"efghij");
        assert_eq!(ring.total(), 10);
        assert_eq!(ring.retained_from(), 2);
        assert_eq!(ring.retained_len(), 8);
        let all = ring.read(0, 100);
        assert_eq!(
            (all.text.as_str(), all.offset, all.next_offset),
            ("cdefghij", 2, 10)
        );
        // One oversized write replaces everything.
        ring.push(b"0123456789ABCDEF");
        assert_eq!(ring.read(0, 100).text, "89ABCDEF");
        assert_eq!(ring.retained_from(), 18);
    }

    #[test]
    fn pages_never_split_a_multibyte_character() {
        let mut ring = LogRing::new(64);
        ring.push("a€b😀c".as_bytes());
        // 1 + 3 + 1 + 4 + 1 bytes: a page of 3 ends inside the euro sign.
        let first = ring.read(0, 3);
        assert_eq!(first.text, "a");
        assert_eq!(first.next_offset, 1);
        let second = ring.read(first.next_offset, 3);
        assert_eq!(second.text, "€");
        let third = ring.read(second.next_offset, 5);
        assert_eq!(third.text, "b😀");
        // Starting inside a character resumes after it.
        let mid = ring.read(2, 100);
        assert_eq!(mid.text, "b😀c");
        assert_eq!(mid.offset, 4);
        // An incomplete tail waits for the rest.
        let mut partial = LogRing::new(64);
        partial.push(&"€".as_bytes()[..2]);
        assert_eq!(partial.read(0, 100).text, "");
        partial.push(&"€".as_bytes()[2..]);
        assert_eq!(partial.read(0, 100).text, "€");
    }

    #[test]
    fn a_settled_log_decodes_a_dangling_character_instead_of_stalling() {
        let mut ring = LogRing::new(64);
        ring.push(b"ok\n");
        ring.push(&"€".as_bytes()[..2]);
        // While running the partial character waits; once settled it is final.
        let running = ring.read(3, 100);
        assert_eq!((running.text.as_str(), running.next_offset), ("", 3));
        let settled = ring.read_with(3, 100, true);
        assert_eq!(settled.text, "\u{FFFD}");
        assert_eq!(settled.next_offset, ring.total());
        assert_eq!(ring.tail_settled(100).text, "ok\n\u{FFFD}");
        // Paging to the end terminates.
        let mut offset = 0;
        for _ in 0..10 {
            let chunk = ring.read_with(offset, 2, true);
            if chunk.next_offset >= chunk.total {
                offset = chunk.next_offset;
                break;
            }
            assert!(chunk.next_offset > offset);
            offset = chunk.next_offset;
        }
        assert_eq!(offset, ring.total());
    }

    #[test]
    fn tail_lines_report_whether_they_are_the_whole_output() {
        let mut ring = LogRing::new(MAX_LOG_BYTES);
        ring.push(b"a\nb\n");
        assert_eq!(
            ring.tail_lines_complete(3),
            (vec!["a".to_owned(), "b".to_owned()], true)
        );
        ring.push(b"c\nd\n");
        assert!(!ring.tail_lines_complete(3).1);
        let mut long = LogRing::new(MAX_LOG_BYTES);
        long.push("x".repeat(MAX_TAIL_LINE_CHARS + 1).as_bytes());
        assert!(!long.tail_lines_complete(3).1);
        let mut rotated = LogRing::new(4);
        rotated.push(b"abcdefgh");
        assert!(!rotated.tail_lines_complete(3).1);
        assert!(LogRing::new(8).tail_lines_complete(3).1);
    }

    #[test]
    fn rotation_that_cuts_a_character_starts_after_it() {
        let mut ring = LogRing::new(5);
        ring.push("€€".as_bytes()); // 6 bytes, retained: last 5 start mid-character
        let chunk = ring.read(0, 100);
        assert_eq!(chunk.text, "€");
        assert_eq!(chunk.offset, 3);
        assert_eq!(chunk.retained_from, 1);
    }

    #[test]
    fn tail_lines_are_bounded_and_skip_blank_lines() {
        let mut ring = LogRing::new(MAX_LOG_BYTES);
        ring.push(b"one\n\ntwo\r\n");
        ring.push("x".repeat(1000).as_bytes());
        ring.push(b"\nlast\n");
        let lines = ring.tail_lines(3);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "two");
        assert_eq!(lines[1].chars().count(), MAX_TAIL_LINE_CHARS);
        assert_eq!(lines[2], "last");
        assert_eq!(ring.tail_lines(10)[0], "one");
    }

    #[test]
    fn summaries_are_one_bounded_line() {
        let argv = vec![
            "sh".to_owned(),
            "-c".to_owned(),
            format!("{}\nrm", "é".repeat(400)),
        ];
        let summary = summarize_command(&argv);
        assert!(!summary.contains('\n'));
        assert_eq!(summary.chars().count(), MAX_SUMMARY_CHARS);
    }

    proptest! {
        // Arbitrary text split into arbitrary chunks: whatever pages come back
        // are valid text, contiguous, and rebuild exactly the retained suffix.
        #[test]
        fn paging_rebuilds_the_retained_suffix(
            text in "\\PC{0,200}",
            cuts in proptest::collection::vec(1usize..17, 0..20),
            capacity in 1usize..64,
            page in 1usize..32,
        ) {
            let bytes = text.as_bytes();
            let mut ring = LogRing::new(capacity);
            let mut at = 0;
            for cut in cuts {
                let end = (at + cut).min(bytes.len());
                ring.push(&bytes[at..end]);
                at = end;
            }
            ring.push(&bytes[at..]);
            prop_assert_eq!(ring.total(), bytes.len() as u64);
            prop_assert!(ring.retained_len() <= capacity);
            let mut offset = 0;
            let mut rebuilt = String::new();
            let mut previous_end = None;
            for _ in 0..2000 {
                let chunk = ring.read(offset, page);
                if let Some(end) = previous_end {
                    // After the first page every page continues exactly.
                    prop_assert_eq!(chunk.offset, end);
                }
                prop_assert!(chunk.next_offset >= chunk.offset);
                rebuilt.push_str(&chunk.text);
                previous_end = Some(chunk.next_offset);
                if chunk.next_offset == offset && chunk.text.is_empty() { break; }
                offset = chunk.next_offset;
            }
            let retained_start = (ring.retained_from() as usize).min(bytes.len());
            let mut start = retained_start;
            while start < bytes.len() && !text.is_char_boundary(start) { start += 1; }
            // Pages stop short of a character a single page cannot hold; the
            // rebuilt text is always a prefix of the retained suffix.
            prop_assert!(text[start..].starts_with(&rebuilt));
        }
    }
}
