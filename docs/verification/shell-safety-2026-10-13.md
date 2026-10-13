# Shell safety live verification — 2026-10-13

Branch `codex/shell-safety`. Live checks are separate from CI. Isolated state:
`HOME` and `--data-dir` point at a scratch directory, the project is a scratch Git repo,
and the real projects folder was not touched. `OPENCODE_KEY` was inherited by the
background server only; it was never printed or stored. Model: `longcat-2.5-preview-free`.

## Commands and outcomes

```sh
cargo build -p themis-desktop
R=<scratch>; export HOME=$R/home LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8
target/debug/themis --data-dir $R/data server start
# user runtime.jsonc rules, saved with: call save_runtime_configuration -
#   {"approval":{"rules":[
#     {"tool":"shell","command":["echo"],"action":"allow"},
#     {"tool":"shell","command":["env"],"action":"allow"}]}}
target/debug/themis --data-dir $R/data thread create $R/proj longcat-2.5-preview-free
target/debug/themis --data-dir $R/data thread send $ID '<call shell: env; then shell: touch denied.txt>' </dev/null
target/debug/themis --data-dir $R/data thread history $ID
target/debug/themis --data-dir $R/data server stop
```

- `auth status` -> `{"go":true}`.
- `shell env` matched the `["env"]` allow rule and ran without a prompt. The recorded
  tool output contained `LANG=en_US.UTF-8`, `LC_ALL=en_US.UTF-8`, `TERM=xterm-256color`
  and no `OPENCODE_KEY`.
- `shell touch denied.txt` matched no rule, fell to `ask`, and the unattended CLI denied it:
  stderr `Tool approval denied: terminal input unavailable. Use chat or a terminal send to approve.`
  Exit status 0 and `denied.txt` did not exist in the project.
- Model summary: the `env` call succeeded, the `touch denied.txt` call was denied.

## Automated coverage (CI)

- `tools::tests::shell_timeout_kills_grandchildren` (unix): grandchild PID gone after timeout.
- `tools::tests::child_env_*`: locale/TERM/SSH_AUTH_SOCK allow and deny matrix; locale
  names survive while `*KEY*`/`*TOKEN*`/`*SECRET*` lookalikes stay blocked.
- `configuration::tests::*command*`, `grant_key_*`, proptest matcher properties.
- `crates/desktop/tests/shell_approval_e2e.rs`: unattended CLI allow-prefix versus deny,
  and an interactive `Always` grant that does not extend to another program.

## Limitations

- Windows: the timeout path uses `taskkill /T /F`; it is compiled and covered by CI
  build but the grandchild-kill behavior was not exercised on Windows (the grandchild
  test is unix-only). Descendants that detach from the tree (or call `setsid` on Unix)
  escape the kill.
- Command matching is by exact token; `/usr/bin/echo` is a different program from `echo`.
