#[test]
fn provider_setup_rejects_redirected_input_without_starting_server() {
    let dir = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir.path())
        .arg("providers")
        .stdin(std::process::Stdio::null())
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires a terminal"));
    assert!(!dir.path().join("server.lock").exists());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn provider_key_is_hidden_and_cancel_restores_terminal() {
    use std::sync::Arc;
    use themis_desktop::{
        secrets::{MemoryStore, SecretStore},
        server::Server,
        state::AppState,
    };
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemoryStore::default());
    let state = AppState::new(dir.path().join("settings.json"), secrets.clone());
    let server = Server::bind(state, dir.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let script = r#"
import os, pty, select, sys, termios, time
binary, directory = sys.argv[1:]
for cancel in [True, False]:
    pid, fd = pty.fork()
    if pid == 0:
        env = dict(os.environ)
        env.pop('OPENCODE_KEY', None)
        os.execve(binary, [binary, '--data-dir', directory, 'providers'], env)
    output = b''
    deadline = time.monotonic() + 15
    def until(marker):
        global output
        while marker not in output:
            assert time.monotonic() < deadline, 'terminal prompt timed out'
            if select.select([fd], [], [], .1)[0]:
                output += os.read(fd, 8192)
    try:
        until(b'Select provider')
        os.write(fd, b'1\n')
        until(b'API key (hidden)')
        while termios.tcgetattr(fd)[3] & termios.ECHO:
            assert time.monotonic() < deadline
            time.sleep(.01)
        os.write(fd, b'synthetic-secret' + (b'\x03' if cancel else b'\x7fx\n'))
        while True:
            assert time.monotonic() < deadline, 'terminal child timed out'
            exited, status = os.waitpid(pid, os.WNOHANG)
            if exited:
                break
            if select.select([fd], [], [], .1)[0]:
                try: output += os.read(fd, 8192)
                except OSError: pass
        assert os.waitstatus_to_exitcode(status) == (1 if cancel else 0), output
        assert b'synthetic-secret' not in output, 'key echoed'
        assert termios.tcgetattr(fd)[3] & termios.ECHO, 'echo was not restored'
    finally:
        try: os.kill(pid, 9)
        except ProcessLookupError: pass
        try: os.waitpid(pid, 0)
        except ChildProcessError: pass
        os.close(fd)
"#;
    let output = std::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(env!("CARGO_BIN_EXE_themis"))
        .arg(dir.path())
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(secrets.get("go").as_deref(), Some("synthetic-secrex"));
    assert!(
        !dir.path().join("settings.json").exists(),
        "no secret in settings"
    );
    task.abort();
}
