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

## Not verified

- Native screenshot of the Tasks tab with a Running row, a Needs attention row (approval or failed automation run) or an Earlier row: the desktop was shared with other agents, so synthetic input was limited to opening the tab once. These groups are covered by Vitest only.
- Clicking Stop or Pause in the native window. Pause is covered by `thread_tasks_cli_e2e` (backend) and by Vitest (bridge dispatch), not by a native click.
- The reduced-motion no-spin rule is a CSS media query; the test only checks the arc element exists.
- Windows and Linux rendering.
