//! The Workspace terminal: the person's own shell in a real PTY, one per project, owned by the
//! server so it outlives switching tab or chat. It runs as the person, with no approvals and no
//! sandbox, so no agent tool reaches it, and it doesn't inherit the secrets the app holds. What it
//! prints stays in memory, bounded; the diagnostics get only that it started and how it ended.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use themis_core::diagnostics::DiagnosticLog;
use tokio::sync::watch;

use super::{canonical_project_dir, AppState};

/// Shells running at once.
const MAX_TERMINALS: usize = 8;
/// One `terminal_input`, in bytes. The page sends longer pastes piece by piece.
const MAX_INPUT: usize = 64 * 1024;
/// Input waiting for a program that isn't reading it.
const INPUT_QUEUE: usize = 64;
/// What is kept for someone coming back; it can grow to twice this between trims.
const MAX_SCROLLBACK: usize = 256 * 1024;
/// One frame to the page.
const MAX_OUTPUT_FRAME: usize = 64 * 1024;
/// Past this a program waits for the terminal, as it would on any slow one.
const MAX_BYTES_PER_SECOND: usize = 8 * 1024 * 1024;
const MAX_COLS: u16 = 500;
const MAX_ROWS: u16 = 200;
/// How long programs get to end on a hangup before they are killed.
const HANGUP_GRACE: Duration = Duration::from_millis(300);
/// How long a shell's last output gets to arrive after it exits.
const DRAIN_GRACE: Duration = Duration::from_secs(1);
/// How long closing waits to see the shell gone.
const CLOSE_WAIT: Duration = Duration::from_secs(3);

/// The terminals of this server, by the folder each runs in.
#[derive(Default)]
pub(super) struct Terminals {
    sessions: Mutex<HashMap<PathBuf, Arc<TerminalSession>>>,
}

/// Output kept for coming back to, with its place in everything printed.
#[derive(Default)]
struct Output {
    text: String,
    start: u64,
}

impl Output {
    fn push(&mut self, text: &str) {
        self.text.push_str(text);
        if self.text.len() > 2 * MAX_SCROLLBACK {
            let mut cut = self.text.len() - MAX_SCROLLBACK;
            while !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            self.text.drain(..cut);
            self.start += cut as u64;
        }
    }

    fn end(&self) -> u64 {
        self.start + self.text.len() as u64
    }

    /// What follows `offset`, a frame at most, and the place after it. A reader that fell behind
    /// what is kept picks up at the oldest text there is.
    fn since(&self, offset: u64) -> (String, u64) {
        let from = usize::try_from(offset.saturating_sub(self.start))
            .unwrap_or(usize::MAX)
            .min(self.text.len());
        let mut to = (from + MAX_OUTPUT_FRAME).min(self.text.len());
        while !self.text.is_char_boundary(to) {
            to -= 1;
        }
        (self.text[from..to].to_owned(), self.start + to as u64)
    }
}

/// Text from `bytes`, holding back a character cut off by the end of a read.
fn decode(carry: &mut Vec<u8>, bytes: &[u8]) -> String {
    carry.extend_from_slice(bytes);
    let mut text = String::new();
    let mut rest: &[u8] = carry;
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                text.push_str(valid);
                rest = &[];
                break;
            }
            Err(error) => {
                let (valid, after) = rest.split_at(error.valid_up_to());
                text.push_str(std::str::from_utf8(valid).unwrap_or_default());
                match error.error_len() {
                    Some(length) => {
                        text.push(char::REPLACEMENT_CHARACTER);
                        rest = &after[length..];
                    }
                    None => {
                        rest = after;
                        break;
                    }
                }
            }
        }
    }
    let kept = rest.to_vec();
    *carry = kept;
    text
}

/// Whether a variable of the app's own environment reaches the person's shell. Their profile runs
/// after this, so what they export themselves is theirs; this keeps out what the app was given.
fn shell_inherits(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    !(upper.starts_with("THEMIS_")
        || ["KEY", "TOKEN", "SECRET", "PASSWORD", "PASSWD", "CREDENTIAL"]
            .iter()
            .any(|word| upper.contains(word)))
}

fn shell_command(root: &Path) -> CommandBuilder {
    #[cfg(unix)]
    let mut command = {
        let shell = std::env::var_os("SHELL")
            .filter(|shell| !shell.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        let mut command = CommandBuilder::new(shell);
        command.arg("-l");
        command
    };
    #[cfg(not(unix))]
    let mut command =
        CommandBuilder::new(std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into()));
    command.cwd(root);
    for (name, _) in std::env::vars_os() {
        if !shell_inherits(&name.to_string_lossy()) {
            command.env_remove(name);
        }
    }
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("TERM_PROGRAM", "Themis");
    // An app started from the Dock has no locale, and the terminal is UTF-8.
    #[cfg(unix)]
    if std::env::var_os("LANG").is_none() {
        command.env("LANG", "en_US.UTF-8");
    }
    command
}

/// Every process of a shell's session (jobs may be in process groups of their own), and everything
/// descended from those, which is how a program that left the session with `setsid` is found.
#[cfg(unix)]
fn session_processes(shell: u32) -> Vec<libc::pid_t> {
    use std::collections::HashSet;
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    if shell <= 1 || shell == std::process::id() {
        return Vec::new();
    }
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let mut found: HashSet<u32> = system
        .processes()
        .values()
        .filter(|process| process.session_id().is_some_and(|id| id.as_u32() == shell))
        .map(|process| process.pid().as_u32())
        .collect();
    // Only while it's there: a shell that has gone may have left its number to something else.
    if system.process(sysinfo::Pid::from_u32(shell)).is_some() {
        found.insert(shell);
    }
    // Each pass adds a generation, and processes can't nest deeper than there are processes.
    loop {
        let before = found.len();
        for process in system.processes().values() {
            if process
                .parent()
                .is_some_and(|parent| found.contains(&parent.as_u32()))
            {
                found.insert(process.pid().as_u32());
            }
        }
        if found.len() == before {
            break;
        }
    }
    found
        .into_iter()
        .filter(|pid| *pid > 1 && *pid != std::process::id())
        .filter_map(|pid| libc::pid_t::try_from(pid).ok())
        .collect()
}

/// Signals processes found a moment ago.
#[cfg(unix)]
fn signal_processes(processes: &[libc::pid_t], signal: libc::c_int) {
    for pid in processes {
        // SAFETY: `kill` takes plain integers, and a process that has gone is only an error.
        unsafe { libc::kill(*pid, signal) };
    }
}

/// One shell.
pub struct TerminalSession {
    output: Mutex<Output>,
    ticks: watch::Sender<u64>,
    input: SyncSender<Vec<u8>>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    #[cfg_attr(not(unix), allow(dead_code))]
    pid: Option<u32>,
    /// How it ended, once it has: its exit code, none when it was closed or killed.
    exit: Mutex<Option<Option<i32>>>,
    closing: AtomicBool,
}

impl TerminalSession {
    fn start(root: &Path, size: PtySize, log: Arc<DiagnosticLog>) -> Result<Arc<Self>, String> {
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|error| format!("Couldn't open a terminal: {error}"))?;
        let mut child = pair
            .slave
            .spawn_command(shell_command(root))
            .map_err(|error| format!("Couldn't start the shell: {error}"))?;
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| format!("Couldn't read the terminal: {error}"))?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|error| format!("Couldn't write to the terminal: {error}"))?;
        let (input, inputs) = sync_channel::<Vec<u8>>(INPUT_QUEUE);
        let (ticks, _) = watch::channel(0);
        let session = Arc::new(Self {
            output: Mutex::default(),
            ticks,
            input,
            master: Mutex::new(Some(pair.master)),
            killer: Mutex::new(child.clone_killer()),
            pid: child.process_id(),
            exit: Mutex::new(None),
            closing: AtomicBool::new(false),
        });
        let _ = log.append("terminal", "started", "info", json!({}));
        // A blocked write waits here, never in the server.
        std::thread::spawn(move || {
            while let Ok(bytes) = inputs.recv() {
                if writer
                    .write_all(&bytes)
                    .and_then(|()| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        let drained = Arc::new(AtomicBool::new(false));
        {
            let session = Arc::clone(&session);
            let drained = Arc::clone(&drained);
            std::thread::spawn(move || {
                session.read_output(&mut reader);
                drained.store(true, Ordering::SeqCst);
            });
        }
        let session_for_wait = Arc::clone(&session);
        std::thread::spawn(move || {
            let status = child.wait();
            let session = session_for_wait;
            // Nothing owns its jobs now, and they would hold the terminal open.
            #[cfg(unix)]
            if let Some(pid) = session.pid {
                signal_processes(&session_processes(pid), libc::SIGKILL);
            }
            let waited = Instant::now();
            while !drained.load(Ordering::SeqCst) && waited.elapsed() < DRAIN_GRACE {
                std::thread::sleep(Duration::from_millis(10));
            }
            session.lock_master().take();
            let code = status
                .ok()
                .filter(|_| !session.closing.load(Ordering::SeqCst))
                .and_then(|status| i32::try_from(status.exit_code()).ok());
            *session.lock_exit() = Some(code);
            session.tick();
            let _ = log.append("terminal", "exited", "info", json!({"code": code}));
        });
        Ok(session)
    }

    fn lock_master(&self) -> std::sync::MutexGuard<'_, Option<Box<dyn MasterPty + Send>>> {
        self.master.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_exit(&self) -> std::sync::MutexGuard<'_, Option<Option<i32>>> {
        self.exit.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_output(&self) -> std::sync::MutexGuard<'_, Output> {
        self.output.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn tick(&self) {
        self.ticks.send_modify(|tick| *tick = tick.wrapping_add(1));
    }

    fn read_output(&self, reader: &mut Box<dyn Read + Send>) {
        let mut buffer = [0u8; 16 * 1024];
        let mut carry = Vec::new();
        let (mut window, mut sent) = (Instant::now(), 0usize);
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 {
                break;
            }
            let text = decode(&mut carry, &buffer[..read]);
            self.lock_output().push(&text);
            self.tick();
            sent += read;
            if sent >= MAX_BYTES_PER_SECOND {
                // Not reading is what makes a program wait.
                let left = Duration::from_secs(1).saturating_sub(window.elapsed());
                std::thread::sleep(left);
                (window, sent) = (Instant::now(), 0);
            } else if window.elapsed() >= Duration::from_secs(1) {
                (window, sent) = (Instant::now(), 0);
            }
        }
    }

    /// Wakes when there is more to read, or it ended.
    #[must_use]
    pub fn updates(&self) -> watch::Receiver<u64> {
        self.ticks.subscribe()
    }

    /// All that is kept, and the place after it.
    #[must_use]
    pub fn scrollback(&self) -> (String, u64) {
        let output = self.lock_output();
        (output.text.clone(), output.end())
    }

    /// What follows `offset`, a frame at most, and the place after it.
    #[must_use]
    pub fn since(&self, offset: u64) -> (String, u64) {
        self.lock_output().since(offset)
    }

    /// How it ended, once it has.
    #[must_use]
    pub fn ended(&self) -> Option<Option<i32>> {
        *self.lock_exit()
    }

    fn describe(&self, root: &Path) -> Value {
        let ended = self.ended();
        json!({
            "root": root,
            "running": ended.is_none(),
            "exit_code": ended.flatten(),
        })
    }

    fn resize(&self, size: PtySize) -> Result<(), String> {
        match self.lock_master().as_ref() {
            Some(master) => master
                .resize(size)
                .map_err(|error| format!("Couldn't resize the terminal: {error}")),
            None => Err("This terminal has ended".to_owned()),
        }
    }

    fn write(&self, bytes: Vec<u8>) -> Result<(), String> {
        if self.ended().is_some() {
            return Err("This terminal has ended".to_owned());
        }
        match self.input.try_send(bytes) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                Err("The program in this terminal isn't reading its input".to_owned())
            }
            Err(TrySendError::Disconnected(_)) => Err("This terminal has ended".to_owned()),
        }
    }

    /// Hangs up on everything in it, kills what stays, and waits to see the shell gone.
    fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            // Found first: once the shell goes, what it started belongs to no one.
            let mut processes = session_processes(pid);
            signal_processes(&processes, libc::SIGHUP);
            std::thread::sleep(HANGUP_GRACE);
            processes.extend(session_processes(pid));
            signal_processes(&processes, libc::SIGKILL);
        }
        #[cfg(not(unix))]
        std::thread::sleep(HANGUP_GRACE);
        let _ = self.killer.lock().unwrap_or_else(|e| e.into_inner()).kill();
        let waited = Instant::now();
        while self.ended().is_none() && waited.elapsed() < CLOSE_WAIT {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn pty_size(cols: u16, rows: u16) -> Result<PtySize, String> {
    if cols == 0 || rows == 0 || cols > MAX_COLS || rows > MAX_ROWS {
        return Err(format!(
            "A terminal is 1 to {MAX_COLS} columns by 1 to {MAX_ROWS} rows"
        ));
    }
    Ok(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })
}

impl Terminals {
    fn open(
        &self,
        root: &Path,
        size: PtySize,
        log: Arc<DiagnosticLog>,
    ) -> Result<Arc<TerminalSession>, String> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(session) = sessions.get(root) {
            if session.ended().is_none() {
                // The page's size is news to a shell it comes back to.
                let _ = session.resize(size);
                return Ok(Arc::clone(session));
            }
        }
        sessions.retain(|_, session| session.ended().is_none());
        if sessions.len() >= MAX_TERMINALS {
            let _ = log.append(
                "terminal",
                "refused",
                "warn",
                json!({"reason": "limit", "limit": MAX_TERMINALS}),
            );
            return Err(format!(
                "{MAX_TERMINALS} terminals are already running: exit one to open another"
            ));
        }
        let session = TerminalSession::start(root, size, log)?;
        sessions.insert(root.to_owned(), Arc::clone(&session));
        Ok(session)
    }

    fn get(&self, root: &Path) -> Option<Arc<TerminalSession>> {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(root)
            .cloned()
    }

    fn take(&self, keep: impl Fn(&Path) -> bool) -> Vec<Arc<TerminalSession>> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let leaving: Vec<PathBuf> = sessions
            .keys()
            .filter(|root| !keep(root))
            .cloned()
            .collect();
        leaving
            .iter()
            .filter_map(|root| sessions.remove(root))
            .collect()
    }
}

/// A path as the terminals know it: resolved, whether or not it still exists.
fn terminal_key(root: &str) -> PathBuf {
    let path = PathBuf::from(root.trim());
    path.canonicalize().unwrap_or(path)
}

impl AppState {
    /// The folder a chat's terminal runs in: its worktree when it has one.
    async fn terminal_root(
        &self,
        project_root: &str,
        thread_id: Option<&str>,
    ) -> Result<PathBuf, String> {
        let root = canonical_project_dir(project_root)?;
        if let Some(thread_id) = thread_id {
            let threads = self.inner.threads.read().await;
            let worktree = threads
                .get(thread_id)
                .filter(|record| {
                    record.project_root.canonicalize().ok().as_deref() == Some(root.as_path())
                })
                .and_then(|record| record.worktree_path.as_ref())
                .and_then(|path| path.canonicalize().ok())
                .filter(|path| path.is_dir());
            if let Some(worktree) = worktree {
                return Ok(worktree);
            }
        }
        Ok(root)
    }

    /// Starts the terminal for this folder, or returns to the one already running.
    pub async fn terminal_open(
        &self,
        project_root: String,
        thread_id: Option<String>,
        cols: u16,
        rows: u16,
    ) -> Result<Value, String> {
        let size = pty_size(cols, rows)?;
        let root = self
            .terminal_root(&project_root, thread_id.as_deref())
            .await?;
        let terminals = Arc::clone(&self.inner.terminals);
        let log = Arc::clone(&self.inner.diagnostics);
        let key = root.clone();
        let session = tokio::task::spawn_blocking(move || terminals.open(&key, size, log))
            .await
            .map_err(|error| error.to_string())??;
        Ok(session.describe(&root))
    }

    /// Whether this folder's terminal is running, and how it ended if not.
    #[must_use]
    pub fn terminal_state(&self, root: &str) -> Value {
        let key = terminal_key(root);
        self.inner.terminals.get(&key).map_or_else(
            || json!({"root": key, "running": false, "exit_code": null}),
            |session| session.describe(&key),
        )
    }

    /// This folder's terminal, to read from.
    #[must_use]
    pub fn terminal_session(&self, root: &str) -> Option<Arc<TerminalSession>> {
        self.inner.terminals.get(&terminal_key(root))
    }

    fn running_terminal(&self, root: &str) -> Result<Arc<TerminalSession>, String> {
        self.terminal_session(root)
            .ok_or_else(|| "This terminal isn't open".to_owned())
    }

    /// Types into the terminal.
    pub fn terminal_input(&self, root: &str, data: &str) -> Result<(), String> {
        if data.len() > MAX_INPUT {
            return Err(format!("Send at most {MAX_INPUT} bytes at a time"));
        }
        self.running_terminal(root)?.write(data.as_bytes().to_vec())
    }

    /// Tells the terminal its new size.
    pub fn terminal_resize(&self, root: &str, cols: u16, rows: u16) -> Result<(), String> {
        let size = pty_size(cols, rows)?;
        self.running_terminal(root)?.resize(size)
    }

    /// Ends this folder's terminal and everything running in it.
    pub async fn terminal_close(&self, root: &str) {
        let key = terminal_key(root);
        self.close_terminals(move |path| path != key, "closed")
            .await;
    }

    /// Ends the terminals in this folder or under it: it is going away.
    pub(super) async fn close_terminals_under(&self, folder: &Path) {
        let folder = folder.to_owned();
        self.close_terminals(move |path| !path.starts_with(&folder), "folder_removed")
            .await;
    }

    /// Ends every terminal: the server is stopping.
    pub async fn shutdown_terminals(&self) {
        self.close_terminals(|_| false, "server_stopped").await;
    }

    async fn close_terminals(
        &self,
        keep: impl Fn(&Path) -> bool + Send + 'static,
        reason: &'static str,
    ) {
        let terminals = Arc::clone(&self.inner.terminals);
        let log = Arc::clone(&self.inner.diagnostics);
        let _ = tokio::task::spawn_blocking(move || {
            let leaving = terminals.take(keep);
            if !leaving.is_empty() {
                let _ = log.append(
                    "terminal",
                    "closed",
                    "info",
                    json!({"reason": reason, "count": leaving.len()}),
                );
            }
            let closing: Vec<_> = leaving
                .into_iter()
                .map(|session| std::thread::spawn(move || session.close()))
                .collect();
            for closed in closing {
                let _ = closed.join();
            }
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn only_what_the_app_holds_stays_out_of_the_shell() {
        for name in [
            "OPENCODE_KEY",
            "anthropic_api_key",
            "GITHUB_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "DB_PASSWORD",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "THEMIS_CATALOG_DATA",
        ] {
            assert!(!shell_inherits(name), "{name}");
        }
        for name in ["PATH", "HOME", "LANG", "SHELL", "SSH_AUTH_SOCK", "EDITOR"] {
            assert!(shell_inherits(name), "{name}");
        }
    }

    #[test]
    fn a_character_cut_by_a_read_waits_for_the_rest() {
        let mut carry = Vec::new();
        let word = "é€😀".as_bytes();
        let mut text = String::new();
        for byte in word {
            text.push_str(&decode(&mut carry, std::slice::from_ref(byte)));
        }
        assert_eq!(text, "é€😀");
        assert!(carry.is_empty());
        assert_eq!(decode(&mut carry, b"a\xffb"), "a\u{fffd}b");
    }

    #[test]
    fn output_is_trimmed_and_a_reader_behind_it_catches_up() {
        let mut output = Output::default();
        output.push(&"é".repeat(2 * MAX_SCROLLBACK));
        assert!(output.text.len() <= 2 * MAX_SCROLLBACK);
        assert_eq!(output.end(), (4 * MAX_SCROLLBACK) as u64);
        let (first, next) = output.since(0);
        assert!(first.len() <= MAX_OUTPUT_FRAME);
        assert!(!first.is_empty());
        assert_eq!(next, output.start + first.len() as u64);
        assert_eq!(output.since(output.end()).0, "");
    }

    #[test]
    fn sizes_have_limits() {
        assert!(pty_size(80, 24).is_ok());
        for (cols, rows) in [(0, 24), (80, 0), (MAX_COLS + 1, 24), (80, MAX_ROWS + 1)] {
            assert!(pty_size(cols, rows).is_err(), "{cols}x{rows}");
        }
    }

    proptest! {
        /// However the bytes are cut into reads, text comes out the same, whole.
        #[test]
        fn any_bytes_decode_the_same_however_they_are_cut(
            bytes in proptest::collection::vec(any::<u8>(), 0..400),
            cut in 0usize..400,
        ) {
            let cut = cut.min(bytes.len());
            let whole = {
                let mut carry = Vec::new();
                let text = decode(&mut carry, &bytes);
                (text, carry)
            };
            let mut carry = Vec::new();
            let mut text = decode(&mut carry, &bytes[..cut]);
            text.push_str(&decode(&mut carry, &bytes[cut..]));
            prop_assert_eq!((text, carry), whole);
        }

        /// Every read lands on a character and within the frame, wherever it starts.
        #[test]
        fn any_output_reads_on_character_boundaries(
            pieces in proptest::collection::vec("\\PC{0,40}", 0..30),
        ) {
            let mut output = Output::default();
            for piece in &pieces {
                output.push(piece);
            }
            let mut offset = 0;
            let mut read = String::new();
            loop {
                let (text, next) = output.since(offset);
                prop_assert!(text.len() <= MAX_OUTPUT_FRAME);
                if text.is_empty() {
                    break;
                }
                read.push_str(&text);
                offset = next;
            }
            prop_assert_eq!(read, output.text);
        }
    }
}
