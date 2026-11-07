//! Watching a running task's process: reaping, deadlines, stop requests, output pumps and group cleanup.

use super::*;

impl BackgroundRegistry {
    pub(super) fn supervise(
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
}

pub(super) fn request_stop(task: &Task) {
    if lock(&task.record).state == TaskState::Running {
        task.stop.store(STOP_REQUESTED, Ordering::SeqCst);
    }
}

pub(super) fn wait_finished(task: &Task, wait: Duration) {
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

pub(super) fn pump<R: Read + Send + 'static>(
    mut pipe: R,
    task: Arc<Task>,
) -> std::thread::JoinHandle<()> {
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
