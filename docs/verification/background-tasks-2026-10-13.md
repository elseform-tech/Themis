# Background shell tasks, 2026-10-13

Branch `codex/background-tasks` (from `main` f02cf38). Automated checks are separate from the live run below.

## Automated checks (macOS)

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten` | every suite passes, 0 failed |
| `cargo test -p themis-core --test background_tasks` | 11 passed (real processes: start/status/log/stop kills grandchildren, natural exit sweeps leftovers, deadline, per-chat/per-app limits, chat privacy, chat deletion, shutdown and crash-restart to `lost`, notifier, tool flow with approval path, cancel flag) |
| `cargo test -p themis-core --lib background` | ring buffer, UTF-8 page boundaries, rotation, tail lines, plus a proptest that pages rebuild the retained suffix |
| `cargo test -p themis-core --lib tools::tests::shell_returns_when_a_backgrounded_grandchild_holds_the_pipes` / `shell_stop_flag_kills_a_running_command_promptly` | pass (regression for gaps a and b) |
| `cargo test -p themis-desktop --test background_tasks_e2e` | 4 passed: scripted-provider run starts and inspects a task, `themis call get_thread_tasks / get_background_task_log / stop_background_task`, chat deletion and `shutdown` kill the task, a second state on the same data dir reports `lost`, `stop_thread` kills a blocking `sleep` |
| `npm --prefix web run lint` / `npm --prefix web test` / `npm --prefix web run build` | pass; 351 tests, including `backgroundTasks.test.tsx` |

## Live run (isolated `HOME` and `--data-dir`, free model `longcat-2.5-preview-free`, YOLO)

Isolated scratch: `HOME=<scratch>/home`, `--data-dir <scratch>/data`, project `<scratch>/proj` (empty git repo). `OPENCODE_KEY` came from the environment and was never printed. The desktop window was not launched.

```sh
T=target/debug/themis; export HOME=<scratch>/home
$T --data-dir <scratch>/data server start
$T --data-dir <scratch>/data call create_thread '{"projectRoot":"<scratch>/proj","provider":"go","model":"longcat-2.5-preview-free"}'
$T --data-dir <scratch>/data call set_thread_approval_mode '{"threadId":"<id>","mode":"yolo"}'
$T --data-dir <scratch>/data thread send <id> "... start python3 -m http.server 8765 with shell background=true; curl it; read the log with background_task ..."
```

1. Before: `lsof -iTCP:8765` empty, `pgrep -f "http.server 8765"` empty.
2. The agent called `shell` with `background: true`, got `task_id` `bg_3453c1`, ran `curl -s -o /dev/null -w %{http_code} http://127.0.0.1:8765/` and reported `200`, then `background_task log` returned `::ffff:127.0.0.1 - - [13/Oct/2026 22:41:30] "GET / HTTP/1.1" 200 -`. The run ended.
3. After the run ended the server was still serving: `lsof` showed Python pid 90389 listening on `*:8765`; `get_thread_tasks` returned `background_tasks[0]` with `state: "running"`, `last_lines` holding the request line, `ended_at: null`.
4. A second message asked the agent to stop it with `background_task stop` and then `status`: it reported `state: stopped`. Afterwards `lsof -iTCP:8765` was empty, `pgrep -fl http.server` showed no 8765 process, `curl` got connection refused, and `get_thread_tasks` showed `state: stopped` with an `ended_at`. (An unrelated `http.server 8123` started by the host user was left alone.)
5. Stop on a blocking command: `thread send --detach` asked for `shell sleep 3151` (blocking). `pgrep` found `sleep 3151` (pid 93341); `themis thread stop <id>` at 22:43:13; the process was gone within the same second and the history shows "Stopped by you. Completed actions remain available for review."
6. Server shutdown: with two tasks running (`http.server 8768` and `8769`), `themis server stop` left both ports free and no `http.server 87xx` process; after `server start`, `get_thread_tasks` listed the earlier tasks as `stopped` (graceful shutdown), with logs kept in `background/<id>.log`.
7. Chat deletion: `call discard_thread` with a running task removed the process, its port and its record (`background/tasks.json` held `"tasks":[]`). This was also covered by the automated `deleting_the_chat_and_shutting_down_the_server_kill_their_tasks`.

Observation: on three attempts the free model answered "started, task_id ..., pid ..." without calling the tool (the diagnostics log has no approval record for those runs, `get_thread_tasks` showed no task, nothing listened). That is model narration, not a runtime behavior; the clean runs above are the evidence. The model also passed `background_timeout_secs: 86400` on its own.

## Native check (lead, real window, computer use)

- `assets/background-tasks-running-native.png`: a live free-model chat started `python3 -m http.server 8799` with `background=true` (task `bg_ef9d07`). After the model run ended, the header shows the running indicator, and Tasks shows the Running row "python3 -m http.server 8799 - Background task - Started 23:04 by agent - ends when this chat closes" with the two curl access-log lines, Stop and Full log.
- `assets/background-tasks-stopped-native.png`: after clicking Stop the row moved to Earlier ("Started 23:04 by agent - Stopped 23:05") with its log kept, and the indicator cleared. `lsof -iTCP:8799` was empty, no `http.server 8799` process remained and curl was refused.
- Full log expands inline ("Hide log"). The port is parsed from recent output, so it is not shown for this task: python prints no "Serving HTTP" line when stdout is a pipe.

## Review follow-ups

- SIGTERM/SIGINT listeners are created once before the accept loop; `server_sigterm_e2e` sends SIGTERM under connection churn and asserts the server exits and kills its background task.
- Lost tasks no longer light the indicator (listed under Earlier with a note); Full log is hidden when the preview is the whole output.
- A finished log ending in a dangling UTF-8 sequence decodes lossily instead of paging forever; a removed chat leaves no orphan `<id>.log`.
- e2e fixtures run an in-process server, so the CLI never auto-starts detached `serve` processes; a fixture assertion checks that none is left.

## Not verified here

- Native rendering after the review follow-ups (hidden Full log, lost under Earlier); the lead's native check above predates them.
- Windows: the process-group handling is compiled out (`taskkill /T /F` path for blocking commands only); background task tests are unix-only and Windows is untested.
- Quality dashboard refresh and `cargo llvm-cov` were not run for this branch.
- Orphans after a hard server crash (SIGKILL/power loss): the next start reports such tasks as `lost`; Themis does not kill them.
