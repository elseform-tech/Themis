# Workspace Tasks tab and task indicator, 2026-10-13

## Automated checks (macOS, worktree `codex/workspace-tasks`)

- `cargo fmt --all -- --check`: pass
- `cargo clippy --workspace --all-targets -- -D warnings`: pass
- `cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten`: all suites pass, including
  `thread_tasks_cli_e2e` (`themis call get_thread_tasks`, per-chat filtering, `set_automation_enabled` pause, unknown thread error),
  the `thread_tasks_report_live_automations_and_finished_runs` unit test and the agent-origin assertions in the `manage_automations` test.
- `npm --prefix web run lint`, `npm --prefix web test` (337+ tests incl. `threadTasks.test.tsx`), `npm --prefix web run build`: pass.

## Native app (real Tauri window, isolated data)

Isolated state: every command ran with `HOME=<scratch>/iso-home`, so the app data dir and projects folder were separate from the real ones.
`OPENCODE_KEY` came from the environment and was never printed.

```sh
(cd web && npm run dev)                                   # vite on :1420
HOME=$ISO target/debug/themis server start
HOME=$ISO target/debug/themis call update_settings '{"patch":{"onboarded":true,"default_model":"longcat-2.5-preview-free"}}'
HOME=$ISO target/debug/themis call create_thread '{"projectRoot":"<default project>","provider":"go","model":"longcat-2.5-preview-free"}'
HOME=$ISO target/debug/themis call create_automation '{"input":{...target_thread_id...,"schedule":{"repeat":"weekdays","time":"09:00",...}}}'
HOME=$ISO target/debug/themis-desktop                     # real native window
HOME=$ISO target/debug/themis call set_thread_approval_mode '{"threadId":"<id>","mode":"yolo"}'
HOME=$ISO target/debug/themis thread send <id> "Use the manage_automations tool to create ... Then write a detailed 600 word explanation ..." --detach
```

Observed with the real free model `longcat-2.5-preview-free`:

- The agent created "Daily test check" through `manage_automations` (after one failed attempt it retried); the feed returned `origin: "agent"`, and the Tasks tab shows "Automation · by agent".
- While the run was live, `get_thread_tasks` returned `run.running: true` and the header showed the stacked indicator (blue arc plus amber clock):
  `docs/verification/assets/workspace-tasks-running-native.png` (native window screenshot).
- Clicking the indicator opened Workspace on the Tasks tab, tab badge "2", group "Scheduled" with both automations, Pause buttons and the footer note:
  `docs/verification/assets/workspace-tasks-tab-scheduled-native.png` (native window screenshot).
- The app, background server and vite were quit afterward.

## Native click-through after review fixes (head `79ad8f5`)

Debug `.app` bundle (`npx -y @tauri-apps/cli@2 build --debug --bundles app` in `crates/desktop`), its `Contents/MacOS/themis-desktop` launched with an isolated `HOME`, so the window carries the `ai.themis.desktop` bundle identity while using scratch data. Clicks were real native clicks through desktop computer use; captures are window-only `screencapture -l`.

- Live `longcat-2.5-preview-free` prompt, then the indicator: Tasks tab shows a Running row ("Chat run · Running now", Stop) above the Scheduled automation (Pause), badge "2":
  `docs/verification/assets/workspace-tasks-running-row-native.png`.
- Clicking Stop: the chat shows "Stopped by you", the Running row disappears, the badge drops to "1" and the indicator keeps only the clock:
  `docs/verification/assets/workspace-tasks-stopped-native.png`.
- Clicking Pause: the row reads "Paused · resume it in Automations", the badge and the header indicator disappear, and `get_thread_tasks` reports the automation `enabled: false`:
  `docs/verification/assets/workspace-tasks-paused-native.png`.

### Temporary self-cancelling watch (live, existing tools only)

In a second chat (YOLO), the agent created "Watch status.txt" through `manage_automations` (`origin: agent`, no calendar schedule, `interval_mins` then set to 2 through `update_automation`). Task: read `status.txt`; if it contains DONE, reply and delete this automation; otherwise reply "still pending". A content-free poller logged `list_automations` and `get_thread_tasks` and flipped the file after two runs.

| Time | Event |
| --- | --- |
| 20:53 | Created; Tasks shows "Every 2 minutes · next 8:55 PM", Automation · by agent (`workspace-temp-watch-scheduled-native.png`) |
| 20:55:50 | Run 1 → "Still pending"; Tasks adds an Earlier row, indicator shows clock + check (`workspace-temp-watch-run1-native.png`) |
| 20:58:31 | Run 2 → "still pending"; poller flips `status.txt` to DONE |
| 21:00:47 | Run 3 → "Task finished - removing this watch"; the agent listed and deleted the automation |
| 21:01:06 | `list_automations` has no automation for the chat; Tasks shows only Earlier runs (`workspace-temp-watch-deleted-native.png`) |

Observations for follow-up work (not regressions of this branch): the agent needed four attempts to create an interval automation because the tool description documents only calendar schedules; runs were 2 min 41 s apart (next run is computed from completion and the scheduler ticks every 30 s); after deletion, Earlier rows fall back to the raw "Automation <name> — <timestamp>" title and lose the "by agent" tag; a chat created through the CLI does not appear in an already open window until it reloads; Stop discards the partially streamed answer text (not yet checked against `main`).

## Not verified

- A native Needs attention row (approval wait or failed automation run); covered by Vitest.
- The reduced-motion no-spin rule is a CSS media query; the test only checks the arc element exists.
- Windows and Linux rendering.
