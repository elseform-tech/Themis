//! Background shell tasks against real processes (unix process groups).
#![cfg(unix)]

use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use themis_core::tools::background::{
    BackgroundRegistry, BackgroundScope, StartSpec, TaskState, STOP_WAIT,
};
use themis_core::tools::{
    boxed_tools_for_chat, AllowAllHook, Approval, ApprovalHook, DenyAllHook, FnHook, ToolAction,
    ToolT,
};

fn spec(thread: &str, script: &str, dir: &Path, timeout_secs: u64) -> StartSpec {
    StartSpec {
        thread_id: thread.to_owned(),
        run_id: None,
        program: "sh".to_owned(),
        args: vec!["-c".to_owned(), script.to_owned()],
        workdir: dir.to_path_buf(),
        timeout_secs,
        sandbox: None,
    }
}

fn alive(pid: libc::pid_t) -> bool {
    // SAFETY: signal 0 only probes for existence.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn pid_from(path: &Path) -> libc::pid_t {
    wait_until("pid file", || {
        std::fs::read_to_string(path).is_ok_and(|text| text.trim().parse::<i32>().is_ok())
    });
    std::fs::read_to_string(path)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

const LOOP: &str = "while true; do sleep 1; done";

#[test]
fn start_reports_output_then_stop_kills_the_whole_group() {
    let dir = tempfile::tempdir().unwrap();
    let registry = BackgroundRegistry::in_memory();
    let pidfile = dir.path().join("grandchild.pid");
    let script = format!(
        "sleep 300 & echo $! > '{}'; echo hello-from-task; echo oops >&2; {LOOP}",
        pidfile.display()
    );
    let started = registry.start(spec("t1", &script, dir.path(), 0)).unwrap();
    assert_eq!(started.record.state, TaskState::Running);
    assert!(started.record.id.starts_with("bg_"));
    let id = started.record.id.clone();
    let grandchild = pid_from(&pidfile);
    wait_until("output", || {
        let (_, chunk) = registry.log("t1", &id, None, 4096).unwrap();
        chunk.text.contains("hello-from-task") && chunk.text.contains("oops")
    });
    let status = registry.status("t1", &id).unwrap();
    assert_eq!(status.record.state, TaskState::Running);
    assert!(status.tail.iter().any(|line| line == "hello-from-task"));

    let stopped = registry.stop("t1", &id, STOP_WAIT).unwrap();
    assert_eq!(stopped.record.state, TaskState::Stopped);
    assert!(stopped.record.ended_at_ms.is_some());
    wait_until("grandchild gone", || !alive(grandchild));
    assert!(!alive(stopped.record.pid as libc::pid_t));
    assert_eq!(registry.running_count(), 0);
    // Stopping again is harmless and the log survives.
    assert_eq!(
        registry.stop("t1", &id, STOP_WAIT).unwrap().record.state,
        TaskState::Stopped
    );
    assert!(registry
        .log("t1", &id, Some(0), 4096)
        .unwrap()
        .1
        .text
        .contains("hello-from-task"));
}

#[test]
fn natural_exit_records_the_code_and_sweeps_leftovers() {
    let dir = tempfile::tempdir().unwrap();
    let registry = BackgroundRegistry::in_memory();
    let pidfile = dir.path().join("left.pid");
    let script = format!(
        "sleep 300 & echo $! > '{}'; echo bye; exit 3",
        pidfile.display()
    );
    let id = registry
        .start(spec("t1", &script, dir.path(), 0))
        .unwrap()
        .record
        .id;
    let leftover = pid_from(&pidfile);
    wait_until("exit", || {
        registry.status("t1", &id).unwrap().record.state != TaskState::Running
    });
    let status = registry.status("t1", &id).unwrap();
    assert_eq!(status.record.state, TaskState::Exited);
    assert_eq!(status.record.exit_code, Some(3));
    assert_eq!(status.tail, vec!["bye".to_owned()]);
    wait_until("leftover swept", || !alive(leftover));
}

#[test]
fn deadline_kills_the_task() {
    let dir = tempfile::tempdir().unwrap();
    let registry = BackgroundRegistry::in_memory();
    let id = registry
        .start(spec("t1", LOOP, dir.path(), 1))
        .unwrap()
        .record
        .id;
    wait_until("deadline", || {
        registry.status("t1", &id).unwrap().record.state == TaskState::TimedOut
    });
    assert!(!alive(
        registry.status("t1", &id).unwrap().record.pid as libc::pid_t
    ));
}

#[test]
fn limits_apply_per_chat_and_per_app_and_free_up_on_stop() {
    let dir = tempfile::tempdir().unwrap();
    let registry = BackgroundRegistry::open_with_limits(None, 2, 3);
    let a1 = registry
        .start(spec("a", LOOP, dir.path(), 0))
        .unwrap()
        .record
        .id;
    registry.start(spec("a", LOOP, dir.path(), 0)).unwrap();
    let error = registry.start(spec("a", LOOP, dir.path(), 0)).unwrap_err();
    assert!(
        error.contains("this chat") && error.contains("limit 2"),
        "{error}"
    );
    registry.start(spec("b", LOOP, dir.path(), 0)).unwrap();
    let error = registry.start(spec("c", LOOP, dir.path(), 0)).unwrap_err();
    assert!(
        error.contains("the app") && error.contains("limit 3"),
        "{error}"
    );
    registry.stop("a", &a1, STOP_WAIT).unwrap();
    registry.start(spec("c", LOOP, dir.path(), 0)).unwrap();
    registry.shutdown(STOP_WAIT);
    assert_eq!(registry.running_count(), 0);
}

#[test]
fn tasks_are_private_to_their_chat() {
    let dir = tempfile::tempdir().unwrap();
    let registry = BackgroundRegistry::in_memory();
    let id = registry
        .start(spec("a", LOOP, dir.path(), 0))
        .unwrap()
        .record
        .id;
    assert!(registry.status("b", &id).is_err());
    assert!(registry.stop("b", &id, STOP_WAIT).is_err());
    assert!(registry.list("b", 0).is_empty());
    assert_eq!(registry.list("a", 0).len(), 1);
    registry.shutdown(STOP_WAIT);
}

#[test]
fn deleting_a_chat_kills_its_tasks_and_forgets_them() {
    let dir = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let registry = BackgroundRegistry::open(Some(store.path().to_path_buf()));
    let pidfile = dir.path().join("p.pid");
    let script = format!("sleep 300 & echo $! > '{}'; {LOOP}", pidfile.display());
    let doomed = registry
        .start(spec("gone", &script, dir.path(), 0))
        .unwrap();
    let kept = registry.start(spec("kept", LOOP, dir.path(), 0)).unwrap();
    let grandchild = pid_from(&pidfile);
    registry.remove_thread("gone", STOP_WAIT);
    assert!(registry.list("gone", 0).is_empty());
    wait_until("group gone", || {
        !alive(grandchild) && !alive(doomed.record.pid as libc::pid_t)
    });
    assert!(!store
        .path()
        .join(format!("{}.log", doomed.record.id))
        .exists());
    assert_eq!(
        registry
            .status("kept", &kept.record.id)
            .unwrap()
            .record
            .state,
        TaskState::Running
    );
    registry.shutdown(STOP_WAIT);
}

#[test]
fn shutdown_stops_everything_and_a_dead_server_leaves_lost_records() {
    let dir = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let registry = BackgroundRegistry::open(Some(store.path().to_path_buf()));
    let id = registry
        .start(spec("t", "echo started; sleep 300", dir.path(), 0))
        .unwrap()
        .record
        .id;
    wait_until("output", || {
        registry.status("t", &id).unwrap().log_bytes > 0
    });

    // A second registry on the same directory while the first still "runs"
    // models a restart after a crash: nothing is claimed to be running.
    let restarted = BackgroundRegistry::open(Some(store.path().to_path_buf()));
    let lost = restarted.status("t", &id).unwrap();
    assert_eq!(lost.record.state, TaskState::Lost);
    assert!(lost.record.ended_at_ms.is_some());
    assert_eq!(restarted.running_count(), 0);

    // Graceful shutdown keeps the log and records the task as stopped.
    registry.shutdown(STOP_WAIT);
    assert_eq!(
        registry.status("t", &id).unwrap().record.state,
        TaskState::Stopped
    );
    let reloaded = BackgroundRegistry::open(Some(store.path().to_path_buf()));
    let status = reloaded.status("t", &id).unwrap();
    assert_eq!(status.record.state, TaskState::Stopped);
    assert_eq!(status.tail, vec!["started".to_owned()]);
}

#[test]
fn notifier_fires_on_start_exit_and_removal() {
    let dir = tempfile::tempdir().unwrap();
    let registry = BackgroundRegistry::in_memory();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&seen);
    registry.set_notifier(Arc::new(move |thread| {
        sink.lock().unwrap().push(thread.to_owned())
    }));
    let id = registry
        .start(spec("t", "exit 0", dir.path(), 0))
        .unwrap()
        .record
        .id;
    wait_until("exit event", || seen.lock().unwrap().len() >= 2);
    assert_eq!(
        registry.status("t", &id).unwrap().record.state,
        TaskState::Exited
    );
    registry.remove_thread("t", STOP_WAIT);
    assert_eq!(seen.lock().unwrap().len(), 3);
}

fn output(value: Result<Value, themis_core::tools::ToolCallError>) -> Value {
    value.unwrap_or_else(|error| panic!("tool failed: {error}"))
}

async fn call(
    tools: &[Box<dyn ToolT>],
    name: &str,
    args: Value,
) -> Result<Value, themis_core::tools::ToolCallError> {
    tools
        .iter()
        .find(|tool| tool.name() == name)
        .unwrap()
        .execute(args)
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn shell_background_flows_through_tools_with_the_same_approval_path() {
    let dir = tempfile::tempdir().unwrap();
    let registry = BackgroundRegistry::in_memory();
    let actions = Arc::new(Mutex::new(Vec::<ToolAction>::new()));
    let seen = Arc::clone(&actions);
    let approvals: Arc<dyn ApprovalHook> = Arc::new(FnHook::new(move |action: &ToolAction| {
        seen.lock().unwrap().push(action.clone());
        Approval::AllowOnce
    }));
    let scope = BackgroundScope::new(Arc::clone(&registry), "chat");
    let tools = boxed_tools_for_chat(dir.path(), approvals, false, Some(scope), None).unwrap();

    let started = output(
        call(
            &tools,
            "shell",
            json!({
                "command": "sh", "args": ["-c", "echo ready; sleep 300"], "background": true
            }),
        )
        .await,
    );
    assert_eq!(started["background"], true);
    let id = started["task_id"].as_str().unwrap().to_owned();
    assert!(
        started.to_string().len() < 600,
        "start result must fit a ledger excerpt"
    );

    // Same ToolAction (argv + grant key) shape as a blocking call.
    let recorded = actions.lock().unwrap().clone();
    assert_eq!(
        recorded[0].command,
        vec!["sh", "-c", "echo ready; sleep 300"]
    );
    assert!(recorded[0].summary.contains("in the background"));
    assert_eq!(recorded[0].tool, "shell");
    assert_eq!(
        recorded[0].grant_key(),
        "shell\0sh\0-c\0echo ready; sleep 300"
    );

    wait_until("ready", || {
        registry.status("chat", &id).unwrap().log_bytes > 0
    });
    let list = output(call(&tools, "background_task", json!({"action": "list"})).await);
    assert_eq!(list["tasks"][0]["task_id"], id);
    let status = output(
        call(
            &tools,
            "background_task",
            json!({"action": "status", "task_id": id}),
        )
        .await,
    );
    assert_eq!(status["state"], "running");
    assert_eq!(status["last_output"], json!(["ready"]));
    let log = output(
        call(
            &tools,
            "background_task",
            json!({"action": "log", "task_id": id, "offset": 0, "max_bytes": 3}),
        )
        .await,
    );
    assert_eq!(log["text"], "rea");
    assert_eq!(log["next_offset"], 3);
    assert_eq!(log["more"], true);
    let rest = output(
        call(
            &tools,
            "background_task",
            json!({"action": "log", "task_id": id, "offset": 3}),
        )
        .await,
    );
    assert_eq!(rest["text"], "dy\n");
    assert_eq!(rest["more"], false);
    let missing = call(
        &tools,
        "background_task",
        json!({"action": "status", "task_id": "bg_nope"}),
    )
    .await;
    assert!(missing
        .unwrap_err()
        .to_string()
        .contains("unknown background task"));
    let stop = output(
        call(
            &tools,
            "background_task",
            json!({"action": "stop", "task_id": id}),
        )
        .await,
    );
    assert_eq!(stop["state"], "stopped");
    registry.shutdown(STOP_WAIT);
}

#[tokio::test(flavor = "multi_thread")]
async fn background_start_is_denied_without_approval_and_without_scope() {
    let dir = tempfile::tempdir().unwrap();
    let registry = BackgroundRegistry::in_memory();
    let denied = boxed_tools_for_chat(
        dir.path(),
        Arc::new(DenyAllHook),
        false,
        Some(BackgroundScope::new(Arc::clone(&registry), "chat")),
        None,
    )
    .unwrap();
    let error = call(
        &denied,
        "shell",
        json!({"command": "sleep", "args": ["300"], "background": true}),
    )
    .await
    .unwrap_err();
    assert!(
        error.to_string().to_lowercase().contains("approval")
            || error.to_string().to_lowercase().contains("denied"),
        "{error}"
    );
    assert_eq!(registry.running_count(), 0);

    let unscoped =
        boxed_tools_for_chat(dir.path(), Arc::new(AllowAllHook), false, None, None).unwrap();
    let error = call(
        &unscoped,
        "shell",
        json!({"command": "sleep", "args": ["300"], "background": true}),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("not available"));
    let error = call(&unscoped, "background_task", json!({"action": "list"}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not available"));
}

#[tokio::test(flavor = "multi_thread")]
async fn setting_the_cancel_flag_kills_a_running_blocking_command() {
    let dir = tempfile::tempdir().unwrap();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let pidfile = dir.path().join("blocking.pid");
    let tools = boxed_tools_for_chat(
        dir.path(),
        Arc::new(AllowAllHook),
        false,
        None,
        Some(Arc::clone(&cancel)),
    )
    .unwrap();
    let script = format!("sleep 300 & echo $! > '{}'; wait", pidfile.display());
    let counter = Arc::new(AtomicUsize::new(0));
    let flag = Arc::clone(&cancel);
    let path = pidfile.clone();
    let trip = std::thread::spawn(move || {
        let _ = pid_from(&path);
        counter.fetch_add(1, Ordering::SeqCst);
        flag.store(true, Ordering::SeqCst);
    });
    let started = Instant::now();
    let error = call(
        &tools,
        "shell",
        json!({"command": "sh", "args": ["-c", script]}),
    )
    .await
    .unwrap_err();
    trip.join().unwrap();
    assert!(error.to_string().contains("stopped by you"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(10));
    let grandchild = pid_from(&pidfile);
    wait_until("blocking group gone", || !alive(grandchild));
}

#[test]
fn a_removed_chat_never_leaves_an_orphan_log_even_when_removal_did_not_wait() {
    let dir = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let registry = BackgroundRegistry::open(Some(store.path().to_path_buf()));
    let started = registry
        .start(spec("gone", "echo hi; exec sleep 300", dir.path(), 0))
        .unwrap();
    wait_until("output", || {
        registry
            .status("gone", &started.record.id)
            .unwrap()
            .log_bytes
            > 0
    });
    // Zero wait: the supervisor is still winding down when the record goes.
    registry.remove_thread("gone", Duration::ZERO);
    wait_until("process gone", || !alive(started.record.pid as libc::pid_t));
    std::thread::sleep(Duration::from_millis(700));
    let logs: Vec<_> = std::fs::read_dir(store.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "log"))
        .collect();
    assert!(logs.is_empty(), "orphan logs: {logs:?}");
    assert!(registry.list("gone", 0).is_empty());
}

#[test]
fn a_finished_task_with_a_dangling_character_pages_to_the_end() {
    let dir = tempfile::tempdir().unwrap();
    let registry = BackgroundRegistry::in_memory();
    let id = registry
        .start(spec("t", r"printf 'ok\342\202'", dir.path(), 0))
        .unwrap()
        .record
        .id;
    wait_until("exit", || {
        registry.status("t", &id).unwrap().record.state != TaskState::Running
    });
    let (_, chunk) = registry.log("t", &id, Some(0), 100).unwrap();
    assert_eq!(chunk.text, "ok\u{FFFD}");
    assert_eq!(chunk.next_offset, chunk.total);
}

#[test]
fn a_task_keeps_the_run_that_started_it_across_a_reload() {
    let dir = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let registry = BackgroundRegistry::open(Some(store.path().to_path_buf()));
    let mut started = spec("t", "echo hi", dir.path(), 0);
    started.run_id = Some("run-7".into());
    let id = registry.start(started).unwrap().record.id;
    let plain = registry
        .start(spec("t", "echo hi", dir.path(), 0))
        .unwrap()
        .record
        .id;
    registry.shutdown(STOP_WAIT);
    let reloaded = BackgroundRegistry::open(Some(store.path().to_path_buf()));
    assert_eq!(
        reloaded.status("t", &id).unwrap().record.run_id.as_deref(),
        Some("run-7")
    );
    // Older records, without a run, still read.
    assert_eq!(reloaded.status("t", &plain).unwrap().record.run_id, None);
}
