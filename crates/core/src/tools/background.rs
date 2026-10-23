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

use super::{async_trait, parse_tool_args, tool_error, ToolCallError, ToolRuntime, ToolT};

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
// Rolling log
// ---------------------------------------------------------------------------

/// Last [`MAX_LOG_BYTES`] of a byte stream with absolute offsets, so readers
/// can page forward and learn how much rotated away.
#[derive(Debug, Clone)]
pub struct LogRing {
    bytes: VecDeque<u8>,
    capacity: usize,
    total: u64,
}

/// One page of a [`LogRing`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogChunk {
    /// Valid UTF-8 text for `offset..next_offset`.
    pub text: String,
    /// Absolute byte offset of the first byte of `text`.
    pub offset: u64,
    /// Offset to pass next to continue; equals `total` when caught up.
    pub next_offset: u64,
    /// Bytes written over the task's lifetime.
    pub total: u64,
    /// Bytes before `retained_from` rotated out of the ring.
    pub retained_from: u64,
}

/// Length of an incomplete UTF-8 sequence at the end of `bytes` (0 when none).
fn incomplete_tail(bytes: &[u8]) -> usize {
    for back in 1..=bytes.len().min(3) {
        let byte = bytes[bytes.len() - back];
        if byte & 0xC0 == 0x80 {
            continue;
        }
        let needed = match byte {
            0xF0..=0xF7 => 4,
            0xE0..=0xEF => 3,
            0xC0..=0xDF => 2,
            _ => 1,
        };
        return if needed > back { back } else { 0 };
    }
    0
}

impl LogRing {
    /// Ring keeping the last `capacity` bytes.
    pub fn new(capacity: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            capacity: capacity.max(1),
            total: 0,
        }
    }

    /// Ring pre-filled with `data` as its whole history.
    pub fn from_bytes(capacity: usize, data: &[u8]) -> Self {
        let mut ring = Self::new(capacity);
        ring.push(data);
        ring
    }

    /// Appends output, dropping the oldest bytes beyond capacity.
    pub fn push(&mut self, data: &[u8]) {
        self.total += data.len() as u64;
        if data.len() >= self.capacity {
            self.bytes.clear();
            self.bytes.extend(&data[data.len() - self.capacity..]);
            return;
        }
        self.bytes.extend(data);
        let excess = self.bytes.len().saturating_sub(self.capacity);
        self.bytes.drain(..excess);
    }

    /// Bytes written over the lifetime of the stream.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Absolute offset of the oldest retained byte.
    pub fn retained_from(&self) -> u64 {
        self.total - self.bytes.len() as u64
    }

    /// Bytes currently retained.
    pub fn retained_len(&self) -> usize {
        self.bytes.len()
    }

    /// Retained bytes as one vector (for persistence).
    pub fn to_vec(&self) -> Vec<u8> {
        self.bytes.iter().copied().collect()
    }

    /// Reads up to `max_bytes` from absolute offset `from` (clamped to what is
    /// retained). Never splits a character: leading continuation bytes are
    /// skipped and an incomplete trailing sequence waits for the next page.
    pub fn read(&self, from: u64, max_bytes: usize) -> LogChunk {
        self.read_with(from, max_bytes, false)
    }

    /// [`read`](Self::read); with `settled` the stream is over, so an
    /// incomplete character at the very end is final and decoded lossily
    /// instead of waiting for bytes that will never arrive.
    pub fn read_with(&self, from: u64, max_bytes: usize, settled: bool) -> LogChunk {
        let retained_from = self.retained_from();
        let mut start = from.clamp(retained_from, self.total);
        let skip = (start - retained_from) as usize;
        // Read a little past the budget so a page always holds one whole
        // character, even when `max_bytes` is smaller than that character.
        let mut slice: Vec<u8> = self
            .bytes
            .iter()
            .skip(skip)
            .take(max_bytes.max(4))
            .copied()
            .collect();
        let leading = slice
            .iter()
            .take_while(|byte| **byte & 0xC0 == 0x80)
            .count();
        // A page that begins inside a character resumes after it.
        slice.drain(..leading);
        start += leading as u64;
        let reaches_end = start + slice.len() as u64 >= self.total;
        let trim = if settled && reaches_end {
            0
        } else {
            incomplete_tail(&slice)
        };
        slice.truncate(slice.len() - trim);
        if slice.len() > max_bytes {
            // Cut back to the last whole character within the budget, but
            // never return an empty page while a character is available.
            let mut end = max_bytes;
            while end > 0 && slice[end] & 0xC0 == 0x80 {
                end -= 1;
            }
            if end == 0 {
                end = 1;
                while end < slice.len() && slice[end] & 0xC0 == 0x80 {
                    end += 1;
                }
            }
            slice.truncate(end);
        }
        let next_offset = start + slice.len() as u64;
        LogChunk {
            text: String::from_utf8_lossy(&slice).into_owned(),
            offset: start,
            next_offset,
            total: self.total,
            retained_from,
        }
    }

    /// The last `max_bytes` of output, aligned to a character boundary.
    pub fn tail(&self, max_bytes: usize) -> LogChunk {
        let from = self.total.saturating_sub(max_bytes as u64);
        self.read(from, max_bytes)
    }

    /// [`tail`](Self::tail) for a finished stream.
    pub fn tail_settled(&self, max_bytes: usize) -> LogChunk {
        let from = self.total.saturating_sub(max_bytes as u64);
        self.read_with(from, max_bytes, true)
    }

    /// Up to `lines` most recent non-empty lines, each bounded in length.
    pub fn tail_lines(&self, lines: usize) -> Vec<String> {
        self.tail_lines_complete(lines).0
    }

    /// [`tail_lines`](Self::tail_lines) plus whether they are the entire
    /// output (nothing rotated out, no line cut, nothing left out).
    pub fn tail_lines_complete(&self, lines: usize) -> (Vec<String>, bool) {
        let chunk = self.tail(8 * 1024);
        let mut complete = self.retained_from() == 0 && chunk.offset == 0;
        let mut found: Vec<String> = chunk
            .text
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .map(|line| {
                complete &= line.chars().count() <= MAX_TAIL_LINE_CHARS;
                line.chars().take(MAX_TAIL_LINE_CHARS).collect()
            })
            .collect();
        let excess = found.len().saturating_sub(lines);
        complete &= excess == 0 && chunk.next_offset >= chunk.total;
        found.drain(..excess);
        (found, complete)
    }
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
    pub program: String,
    pub args: Vec<String>,
    pub workdir: PathBuf,
    /// Per-task deadline in seconds; 0 selects the default.
    pub timeout_secs: u64,
    /// Sandbox the task runs in (the run's captured plan); `None` runs it as is.
    pub sandbox: Option<Arc<crate::sandbox::SandboxPlan>>,
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
        let prepared = super::prepare_command(&spec.program, &spec.args, spec.sandbox.as_deref())
            .map_err(|error| super::spawn_failure(&spec.program, &spec.args, &error))?;
        let mut command = prepared.command();
        command
            .current_dir(&spec.workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .envs(super::scrubbed_env());
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

    fn supervise(
        &self,
        task: &Arc<Task>,
        mut child: std::process::Child,
        readers: Vec<std::thread::JoinHandle<()>>,
        deadline: Instant,
    ) {
        let (state, exit_code) = loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    // The program is gone; stop anything it left in its group.
                    sweep_group(child.id());
                    break (TaskState::Exited, status.code());
                }
                Ok(None) => {}
                Err(_) => break (TaskState::Lost, None),
            }
            let timed_out = Instant::now() >= deadline;
            if task.stop.load(Ordering::SeqCst) == STOP_REQUESTED || timed_out {
                super::kill_process_tree(&mut child);
                let status = child.wait().ok();
                break (
                    if timed_out && task.stop.load(Ordering::SeqCst) != STOP_REQUESTED {
                        TaskState::TimedOut
                    } else {
                        TaskState::Stopped
                    },
                    status.and_then(|status| status.code()),
                );
            }
            std::thread::sleep(SUPERVISOR_TICK);
        };
        // Pipes close with the group; a process that escaped it (setsid) may
        // keep them open, so wait only briefly and detach.
        let grace = Instant::now() + READER_GRACE;
        while Instant::now() < grace && !readers.iter().all(|reader| reader.is_finished()) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let thread_id = lock(&task.record).thread_id.clone();
        let id = lock(&task.record).id.clone();
        // Only a task that is still registered keeps a log file: a chat removed
        // while this supervisor was winding down must not leave an orphan.
        // Holding the registry lock orders this against `remove_thread`.
        if let Some(path) = self.log_path(&id) {
            let tasks = lock(&self.tasks);
            if tasks.iter().any(|registered| Arc::ptr_eq(registered, task)) {
                if let Some(dir) = &self.dir {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(path, lock(&task.log).to_vec());
            }
        }
        {
            let mut record = lock(&task.record);
            record.state = state;
            record.exit_code = exit_code;
            record.ended_at_ms = Some(now_ms());
        }
        self.prune_finished(&thread_id);
        self.persist();
        self.notify(&thread_id);
        task.settled.store(true, Ordering::SeqCst);
        // Taking the lock orders the flag before any waiter's next check.
        drop(lock(&task.record));
        task.finished.notify_all();
    }

    fn prune_finished(&self, thread_id: &str) {
        let mut dropped = Vec::new();
        {
            let mut tasks = lock(&self.tasks);
            let mut finished: Vec<(u64, String)> = tasks
                .iter()
                .map(|task| task.record())
                .filter(|record| {
                    record.thread_id == thread_id && record.state != TaskState::Running
                })
                .map(|record| {
                    (
                        record.ended_at_ms.unwrap_or(record.started_at_ms),
                        record.id,
                    )
                })
                .collect();
            if finished.len() <= MAX_FINISHED_PER_THREAD {
                return;
            }
            finished.sort();
            let excess = finished.len() - MAX_FINISHED_PER_THREAD;
            dropped.extend(finished.into_iter().take(excess).map(|(_, id)| id));
            tasks.retain(|task| !dropped.contains(&lock(&task.record).id));
        }
        for id in dropped {
            if let Some(path) = self.log_path(&id) {
                let _ = std::fs::remove_file(path);
            }
        }
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

fn request_stop(task: &Task) {
    if lock(&task.record).state == TaskState::Running {
        task.stop.store(STOP_REQUESTED, Ordering::SeqCst);
    }
}

fn wait_finished(task: &Task, wait: Duration) {
    let deadline = Instant::now() + wait;
    let mut record = lock(&task.record);
    while !task.settled.load(Ordering::SeqCst) {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return;
        };
        record = task
            .finished
            .wait_timeout(record, remaining)
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .0;
    }
}

fn pump<R: Read + Send + 'static>(mut pipe: R, task: Arc<Task>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => lock(&task.log).push(&buffer[..read]),
            }
        }
    })
}

/// Stops what is left of `pgid` after its leader exited: SIGTERM, a short
/// grace, then SIGKILL, only while the group still has members.
fn sweep_group(pgid: u32) {
    #[cfg(unix)]
    {
        let pgid = pgid as libc::pid_t;
        // SAFETY: signal 0 only probes for existence.
        let exists = |pgid| unsafe { libc::killpg(pgid, 0) } == 0;
        if !exists(pgid) {
            return;
        }
        // SAFETY: the group was created by the spawn that owns this task.
        unsafe { libc::killpg(pgid, libc::SIGTERM) };
        let grace = Instant::now() + Duration::from_millis(300);
        while Instant::now() < grace && exists(pgid) {
            std::thread::sleep(Duration::from_millis(10));
        }
        if exists(pgid) {
            // SAFETY: as above.
            unsafe { libc::killpg(pgid, libc::SIGKILL) };
        }
    }
    #[cfg(not(unix))]
    let _ = pgid;
}

/// Directory used to persist background tasks beside the app data.
pub fn storage_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("background")
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// The chat a tool belongs to plus the registry holding its tasks.
#[derive(Clone)]
pub struct BackgroundScope {
    pub registry: Arc<BackgroundRegistry>,
    pub thread_id: String,
}

fn blocking_error(error: tokio::task::JoinError) -> ToolCallError {
    tool_error(format!("background_task: worker failed: {error}"))
}

impl BackgroundScope {
    /// Scope for `thread_id` backed by `registry`.
    pub fn new(registry: Arc<BackgroundRegistry>, thread_id: impl Into<String>) -> Self {
        Self {
            registry,
            thread_id: thread_id.into(),
        }
    }

    /// Starts a background task (the caller already obtained approval).
    pub(super) async fn start(
        &self,
        program: String,
        args: Vec<String>,
        workdir: PathBuf,
        timeout_secs: u64,
        sandbox: Option<Arc<crate::sandbox::SandboxPlan>>,
    ) -> Result<Value, ToolCallError> {
        let registry = Arc::clone(&self.registry);
        let sandboxed = sandbox.as_ref().is_some_and(|plan| plan.is_active());
        let spec = StartSpec {
            thread_id: self.thread_id.clone(),
            program,
            args,
            workdir,
            timeout_secs,
            sandbox,
        };
        let view = tokio::task::spawn_blocking(move || registry.start(spec))
            .await
            .map_err(blocking_error)?
            .map_err(|error| tool_error(format!("shell: {error}")))?;
        Ok(json!({
            "background": true,
            "task_id": view.record.id,
            "state": view.record.state,
            "pid": view.record.pid,
            "command": view.summary,
            "timeout_secs": view.record.timeout_secs,
            "sandboxed": sandboxed,
            "next": crate::prompts::TOOL_BACKGROUND_NEXT,
        }))
    }
}

fn default_offset() -> i64 {
    -1
}

/// Arguments for [`BackgroundTool`].
#[derive(Debug, Clone, Serialize, Deserialize, ToolInput)]
pub struct BackgroundArgs {
    /// `list`, `status`, `log` or `stop`.
    #[input(description = "list | status | log | stop")]
    pub action: String,
    /// Task id returned by shell with background=true.
    #[serde(default)]
    #[input(description = "task_id from shell background=true (not needed for list)")]
    pub task_id: String,
    /// Byte offset to read the log from; negative reads the newest output.
    #[serde(default = "default_offset")]
    #[input(
        description = "log only: byte offset to read from (next_offset of the previous page); -1 (default) returns the newest output"
    )]
    pub offset: i64,
    /// Largest log page, in bytes.
    #[serde(default)]
    #[input(description = "log only: page size in bytes (default 8192, max 16384)")]
    pub max_bytes: u64,
}

/// Inspects and stops this chat's background shell tasks.
///
/// No approval prompt: it only reaches tasks this chat already started with an
/// approved `shell` call, and the output is bounded.
#[tool(
    name = "background_task",
    description = "Inspect or stop background programs started with shell background=true. list: this chat's tasks. status: state, exit code and the last output lines. log: bounded page of the rolling log (last 64 KiB; page with next_offset). stop: kills the program and everything it started.",
    input = BackgroundArgs,
)]
pub struct BackgroundTool {
    scope: Option<BackgroundScope>,
}

impl BackgroundTool {
    /// Tool for one chat; without a scope every call reports it is unavailable.
    pub fn new(scope: Option<BackgroundScope>) -> Self {
        Self { scope }
    }
}

#[async_trait]
impl ToolRuntime for BackgroundTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let BackgroundArgs {
            action,
            task_id,
            offset,
            max_bytes,
        } = parse_tool_args("background_task", args)?;
        let scope = self.scope.clone().ok_or_else(|| {
            tool_error("background_task: background tasks are not available in this context".into())
        })?;
        if action != "list" && task_id.trim().is_empty() {
            return Err(tool_error(format!(
                "background_task: '{action}' needs a task_id (see action=list)"
            )));
        }
        let fail = |error: String| tool_error(format!("background_task: {error}"));
        tokio::task::spawn_blocking(move || {
            let registry = &scope.registry;
            let thread = scope.thread_id.as_str();
            match action.as_str() {
                "list" => {
                    let tasks: Vec<Value> = registry
                        .list(thread, 0)
                        .into_iter()
                        .map(|view| {
                            json!({
                                "task_id": view.record.id,
                                "state": view.record.state,
                                "exit_code": view.record.exit_code,
                                "command": view.summary,
                                "log_bytes": view.log_bytes,
                            })
                        })
                        .collect();
                    Ok(json!({"tasks": tasks, "running": registry.running_count()}))
                }
                "status" => {
                    let view = registry.status(thread, &task_id).map_err(fail)?;
                    Ok(json!({
                        "task_id": view.record.id,
                        "state": view.record.state,
                        "exit_code": view.record.exit_code,
                        "command": view.summary,
                        "log_bytes": view.log_bytes,
                        "last_output": view.tail,
                    }))
                }
                "log" => {
                    let size = match max_bytes {
                        0 => DEFAULT_LOG_PAGE_BYTES,
                        bytes => usize::try_from(bytes).unwrap_or(MAX_LOG_PAGE_BYTES),
                    };
                    let offset = u64::try_from(offset).ok();
                    let (record, chunk) =
                        registry.log(thread, &task_id, offset, size).map_err(fail)?;
                    Ok(json!({
                        "task_id": record.id,
                        "state": record.state,
                        "exit_code": record.exit_code,
                        "text": chunk.text,
                        "offset": chunk.offset,
                        "next_offset": chunk.next_offset,
                        "total_bytes": chunk.total,
                        "oldest_retained_offset": chunk.retained_from,
                        "more": chunk.next_offset < chunk.total,
                    }))
                }
                "stop" => {
                    let view = registry.stop(thread, &task_id, STOP_WAIT).map_err(fail)?;
                    Ok(json!({
                        "task_id": view.record.id,
                        "state": view.record.state,
                        "exit_code": view.record.exit_code,
                        "last_output": view.tail,
                    }))
                }
                other => Err(tool_error(format!(
                    "background_task: unknown action '{other}' (use list, status, log or stop)"
                ))),
            }
        })
        .await
        .map_err(blocking_error)?
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
