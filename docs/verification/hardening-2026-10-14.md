# Hardening verification, 2026-10-14

Branch `codex/hardening`. Automatic checks are labelled separately from the live,
manual evidence.

## Automatic checks

| Item | Test |
| --- | --- |
| Shell schema and self-correcting error | `tools::tests::shell_command_with_whole_command_line_explains_how_to_split_it`, `spawn_failure_hints_only_for_missing_programs_with_whitespace`, property test `spawn_failure_props::never_panics_and_hint_round_trips`; scripted provider test `shell_call_with_whole_command_line_self_corrects_from_the_error` (runtime_e2e) |
| Flag-sensitive approvals | `configuration::tests::destructive_variants_get_their_own_grant_key`, `allow_rule_for_git_push_does_not_cover_forced_push`, `approving_git_push_once_always_still_asks_for_force_push`, property tests for the key function, `tools::tests::git_tool_reports_argv_and_escalates_forced_variants`; CLI e2e `allowing_git_push_does_not_allow_a_forced_push_unattended` (desktop `shell_approval_e2e`) |
| Windows process-tree kill | `#[cfg(windows)]` `shell_timeout_kills_the_grandchild_process_tree_on_windows` and `background_tasks_windows::stopping_a_background_task_kills_its_grandchild_on_windows` (run by CI on windows-latest only; not run locally on macOS) |
| Stale automation snapshot | `state::automation::stale_snapshot_tests::a_stale_due_snapshot_does_not_rerun_the_check`; verified to fail when the post-guard re-read is replaced by the stale snapshot |
| Flaky `hard_cap_returns_natural_handoff_without_failing` | Root cause: the toolkit `write_file` append writes through tokio's buffered `File` and never flushes, so the call can return before the bytes land (reader sees an empty file). Fix: the gated tool waits for the appended bytes. Regression `tools::tests::gated_append_is_durable_when_the_call_returns` fails without the fix (empty reads) and passes with it. The CI flake was not reproducible locally in 150 idle and 150 CPU-loaded runs before the fix; after the fix the test passed 100/100 runs |
| PR #15 gap | `ratio_rising_on_the_last_turn_compacts_before_the_handoff` now asserts the compaction summary `max_tokens` (610 with the refresh, 2275 when the `raw_budget` refresh line is reverted; verified by reverting it locally) |

`cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten`:
596 passed, 0 failed, 3 ignored.

## Live check (manual, not part of CI)

Isolated `HOME` and `--data-dir` under a scratch directory, `themis serve` stopped
afterwards, free model `longcat-2.5-preview-free` through `OPENCODE_KEY` (value never
printed), throwaway local repo with a bare remote.

1. User rule `{"tool":"shell","command":["git","push"],"action":"allow"}`, unattended
   `thread send` asking for `git push origin HEAD:refs/heads/plain` then
   `git push --force origin HEAD:refs/heads/forced`. The plain push ran and created
   `plain` on the remote. The forced push was not run ("Tool approval denied: terminal
   input unavailable"); the remote has only `plain`.
2. Rule allowing `shell` and `background_task`; "start a python web server on port 8797
   in the background". With the new schema the model sent `command: "python3"`,
   `args: ["-m","http.server","8797"]`, `background: true` on the first try; the
   server answered HTTP 200 and the port was closed after the server stopped. The
   self-correcting error did not trigger live, so it is covered by the scripted test only.

## Limitations

- Windows tests are unverified locally; CI on windows-latest is the evidence. The Windows
  kill still relies on `taskkill /T /F` (a descendant detached from its parent is not reached).
- A broad rule without `command` (for example `{"tool":"shell","action":"allow"}`) remains
  explicit trust and still covers destructive variants.
- Tool-call-time risk escalation for destructive variants applies to the `git` tool only;
  `shell` stays risk `execute`.
