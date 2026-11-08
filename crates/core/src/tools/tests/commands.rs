use super::*;

#[cfg(unix)]
#[tokio::test]
async fn shell_timeout_kills_long_running_command() {
    let (_tmp, root) = rooted_case();
    let tool = ShellTool::new(root, allow_all()).with_timeout(Duration::from_secs(1));
    let start = Instant::now();
    let err = tool
        .execute(json!({ "command": "sleep", "args": ["300"] }))
        .await
        .unwrap_err();
    let message = err.to_string().to_lowercase();
    assert!(message.contains("timed out"), "{message}");
    assert!(message.contains("1s"), "{message}");
    assert!(
        start.elapsed() < Duration::from_secs(30),
        "kill took too long: {:?}",
        start.elapsed()
    );
}

/// Number of `ping.exe` processes whose command line carries `-n <marker> `.
#[cfg(windows)]
fn windows_ping_count(marker: &str) -> usize {
    let script = format!(
        "@(Get-CimInstance Win32_Process -Filter \"Name='ping.exe'\" | \
             Where-Object {{ $_.CommandLine -like '*-n {marker} *' }}).Count"
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .expect("powershell must be available");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("unexpected powershell output: {out:?}"))
}

/// Windows counterpart of the Unix process-group test: `cmd /c ping` makes
/// `ping.exe` a grandchild of the tool, and the timeout must end it too.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn shell_timeout_kills_the_grandchild_process_tree_on_windows() {
    let (_tmp, root) = rooted_case();
    let tool = ShellTool::new(root, allow_all()).with_timeout(Duration::from_secs(40));
    let marker = "3101";
    let running = tokio::spawn(async move {
        tool.execute(json!({
            "command": "cmd",
            "args": ["/c", format!("ping -n {marker} 127.0.0.1")],
        }))
        .await
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    while windows_ping_count(marker) == 0 {
        assert!(Instant::now() < deadline, "grandchild never started");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let message = running.await.unwrap().unwrap_err().to_string();
    assert!(message.contains("timed out"), "{message}");
    let deadline = Instant::now() + Duration::from_secs(30);
    while windows_ping_count(marker) != 0 {
        assert!(Instant::now() < deadline, "grandchild survived the timeout");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Windows counterpart of the leftover tests: `start /b` leaves `ping.exe`
/// running after `cmd` exits; the command's job ends it when the call returns.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn shell_stops_a_program_left_running_on_windows() {
    let (_tmp, root) = rooted_case();
    let marker = "3147";
    let result = ShellTool::new(root, allow_all())
        .execute(json!({
            "command": "cmd",
            "args": ["/c", format!("start /b ping -n {marker} 127.0.0.1 >nul & echo launched")],
        }))
        .await
        .unwrap();
    let text = result["output"].as_str().unwrap();
    assert!(text.contains("launched"), "{text}");
    assert!(text.contains("Themis stopped them"), "{text}");
    let deadline = Instant::now() + Duration::from_secs(30);
    while windows_ping_count(marker) != 0 {
        assert!(Instant::now() < deadline, "ping outlived the shell call");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn shell_env_is_scrubbed() {
    let (_tmp, root) = rooted_case();
    std::env::set_var("THEMIS_SECRET_PROBE", "should-not-appear-98765");
    let result = ShellTool::new(root, allow_all())
        .execute(json!({ "command": "env", "args": [] }))
        .await
        .unwrap();
    std::env::remove_var("THEMIS_SECRET_PROBE");
    let output = result["output"].as_str().unwrap();
    assert!(
        !output.contains("THEMIS_SECRET_PROBE"),
        "sentinel leaked: {output}"
    );
    assert!(
        !output.contains("should-not-appear-98765"),
        "sentinel value leaked: {output}"
    );
    assert!(output.contains("PATH="), "PATH missing: {output}");
}

#[test]
fn child_env_blocklist_wins_over_allowlist() {
    for secret in [
        "API_KEY",
        "api_key",
        "Github_Token",
        "MY_SECRET",
        "THEMIS_SECRET_PROBE",
        "SECRET_PATH",
    ] {
        assert!(!child_env_allowed(secret), "{secret} must be blocked");
    }
    for allowed in ["PATH", "Path", "TMPDIR", "TEMP", "TMP"] {
        assert!(child_env_allowed(allowed), "{allowed} must pass");
    }
    #[cfg(not(windows))]
    for allowed in ["HOME", "USER", "LOGNAME"] {
        assert!(child_env_allowed(allowed), "{allowed} must pass");
    }
    #[cfg(windows)]
    for allowed in [
        "SYSTEMROOT",
        "SYSTEMDRIVE",
        "USERNAME",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
    ] {
        assert!(child_env_allowed(allowed), "{allowed} must pass");
    }
    assert!(!child_env_allowed("RANDOM_UNRELATED_VAR"));
    #[cfg(not(windows))]
    assert!(child_env_allowed("SSH_AUTH_SOCK"));
    #[cfg(windows)]
    assert!(!child_env_allowed("SSH_AUTH_SOCK"));
}

#[test]
fn child_env_passes_locale_and_term_but_not_lookalikes() {
    for allowed in [
        "LANG",
        "lang",
        "LC_ALL",
        "LC_CTYPE",
        "lc_messages",
        "LC_TIME",
        "TERM",
    ] {
        assert!(child_env_allowed(allowed), "{allowed} must pass");
    }
    for denied in [
        "LANGUAGE_SERVER",
        "LCALL",
        "TERMINAL",
        "TERM_KEY",
        "LC_SECRET",
        "LC_TOKEN",
        "LANG_KEY",
        "SSH_AUTH_SOCKET",
    ] {
        assert!(!child_env_allowed(denied), "{denied} must be blocked");
    }
}

/// Reads the pid a test script wrote to `pidfile`.
#[cfg(unix)]
fn read_pid(pidfile: &std::path::Path) -> libc::pid_t {
    std::fs::read_to_string(pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

/// Whether `pid` is gone within `within` (bounded by a deadline, not a count).
#[cfg(unix)]
fn gone_within(pid: libc::pid_t, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        // SAFETY: signal 0 only probes for existence.
        if unsafe { libc::kill(pid, 0) } != 0 {
            return true;
        }
        if Instant::now() >= deadline {
            // SAFETY: cleanup of the helper process the test started.
            unsafe { libc::kill(pid, libc::SIGKILL) };
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A program left behind with `&` that keeps the output pipes open: the call
/// still returns promptly, and the program is stopped, not left running.
#[cfg(unix)]
#[tokio::test]
async fn shell_stops_a_backgrounded_grandchild_that_holds_the_pipes() {
    let (_tmp, root) = rooted_case();
    let pidfile = root.join("held.pid");
    let script = format!(
        "sleep 300 & echo $! > '{}'; echo started",
        pidfile.display()
    );
    let tool = ShellTool::new(root, allow_all());
    let began = Instant::now();
    let result = tool
        .execute(json!({ "command": "sh", "args": ["-c", script] }))
        .await
        .unwrap();
    let elapsed = began.elapsed();
    let pid = read_pid(&pidfile);
    let gone = gone_within(pid, Duration::from_secs(5));
    assert!(elapsed < Duration::from_secs(10), "hung for {elapsed:?}");
    assert!(gone, "backgrounded sleep {pid} outlived the shell call");
    assert_eq!(result["success"], true);
    let text = result["output"].as_str().unwrap();
    assert!(text.contains("started"), "{text}");
    assert!(text.contains("Themis stopped them"), "{text}");
    assert!(!text.contains("still holds its output open"), "{text}");
}

/// The goal-run leak: `nohup server > log 2>&1 &` holds no pipe, so the call
/// returned at once and the server lived on after the chat and the app.
#[cfg(unix)]
#[tokio::test]
async fn shell_stops_a_nohup_program_left_running_after_the_command() {
    let (_tmp, root) = rooted_case();
    let pidfile = root.join("nohup.pid");
    let log = root.join("nohup.log");
    let script = format!(
        "nohup sleep 300 > '{}' 2>&1 & echo $! > '{}'; echo launched",
        log.display(),
        pidfile.display()
    );
    let result = ShellTool::new(root, allow_all())
        .execute(json!({ "command": "sh", "args": ["-c", script] }))
        .await
        .unwrap();
    let pid = read_pid(&pidfile);
    let gone = gone_within(pid, Duration::from_secs(5));
    assert!(gone, "nohup sleep {pid} outlived the shell call");
    let text = result["output"].as_str().unwrap();
    assert!(text.contains("launched"), "{text}");
    assert!(text.contains("background=true"), "{text}");
}

/// A command that leaves nothing behind gets no note.
#[cfg(unix)]
#[tokio::test]
async fn shell_without_leftovers_adds_no_note() {
    let (_tmp, root) = rooted_case();
    let result = ShellTool::new(root, allow_all())
        .execute(
            json!({ "command": "sh", "args": ["-c", "(sleep 0.1; echo inner) & wait; echo done"] }),
        )
        .await
        .unwrap();
    let text = result["output"].as_str().unwrap();
    assert!(text.contains("inner") && text.contains("done"), "{text}");
    assert!(!text.contains("[note:"), "{text}");
}

#[cfg(unix)]
#[tokio::test]
async fn shell_stop_flag_kills_a_running_command_promptly() {
    let (_tmp, root) = rooted_case();
    let cancel = Arc::new(AtomicBool::new(false));
    let tool = ShellTool::new(root, allow_all()).with_cancel(Arc::clone(&cancel));
    let flag = Arc::clone(&cancel);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        flag.store(true, Ordering::SeqCst);
    });
    let began = Instant::now();
    let error = tool
        .execute(json!({ "command": "sleep", "args": ["60"] }))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("stopped by you"), "{error}");
    assert!(began.elapsed() < Duration::from_secs(5));
}

#[cfg(unix)]
#[tokio::test]
async fn shell_timeout_kills_grandchildren() {
    let (tmp, root) = rooted_case();
    let pidfile = root.join("grandchild.pid");
    let script = format!("sleep 300 & echo $! > '{}'; wait", pidfile.display());
    let tool = ShellTool::new(root, allow_all()).with_timeout(Duration::from_secs(1));
    let err = tool
        .execute(json!({ "command": "sh", "args": ["-c", script] }))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("timed out"));
    let pid: libc::pid_t = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // The orphaned grandchild must be gone (allow a moment for reaping).
    let mut alive = true;
    for _ in 0..100 {
        // SAFETY: signal 0 only probes for existence.
        alive = unsafe { libc::kill(pid, 0) } == 0;
        if !alive {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(tmp);
    assert!(!alive, "grandchild {pid} survived the timeout");
}

#[cfg(unix)]
#[tokio::test]
async fn shell_child_sees_locale_variables() {
    let (_tmp, root) = rooted_case();
    std::env::set_var("LC_THEMIS_PROBE", "probe-locale");
    let result = ShellTool::new(root, allow_all())
        .execute(json!({ "command": "env", "args": [] }))
        .await
        .unwrap();
    std::env::remove_var("LC_THEMIS_PROBE");
    assert!(result["output"]
        .as_str()
        .unwrap()
        .contains("LC_THEMIS_PROBE=probe-locale"));
}

#[test]
fn management_approvals_separate_read_mutation_and_changed_execution() {
    assert_eq!(
        integration_risk("save_skill", &json!({"action":"list"})),
        Some(RiskLevel::Write)
    );
    assert_eq!(
        integration_risk("manage_integrations", &json!({"action":"list"})),
        Some(RiskLevel::Read)
    );
    for action in [
        "scan_marketplace",
        "add_marketplace",
        "inspect_marketplace",
        "inspect_repository",
    ] {
        assert_eq!(
            integration_risk("manage_integrations", &json!({"action":action})),
            Some(RiskLevel::Network)
        );
    }
    // Cached catalog requests can still fetch when the source cache is absent.
    assert_eq!(
        integration_risk(
            "manage_integrations",
            &json!({"action":"catalog","refresh":false})
        ),
        Some(RiskLevel::Network)
    );
    // Preview does not install, but uncached sources can fetch repositories.
    assert_eq!(
        integration_risk(
            "manage_integrations",
            &json!({"action":"preview","refresh":false})
        ),
        Some(RiskLevel::Network)
    );
    assert_eq!(
        integration_risk(
            "manage_integrations",
            &json!({"action":"preview_repository"})
        ),
        Some(RiskLevel::Network)
    );
    assert_eq!(
        integration_risk("manage_integrations", &json!({"action":"delete"})),
        Some(RiskLevel::Destructive)
    );
    assert_eq!(
        integration_risk("manage_integrations", &json!({"action":"remove_component"})),
        Some(RiskLevel::Destructive)
    );
    assert_eq!(
        integration_risk("manage_integrations", &json!({"action":"test_hook"})),
        Some(RiskLevel::Execute)
    );
    assert_ne!(
        integration_approval_identity("manage_integrations", &json!({"action":"list"})),
        integration_approval_identity("manage_integrations", &json!({"action":"save"}))
    );
    assert_ne!(
        integration_approval_identity(
            "manage_integrations",
            &json!({"action":"test_hook","hook":{"command":"true"}})
        ),
        integration_approval_identity(
            "manage_integrations",
            &json!({"action":"test_hook","hook":{"command":":"}})
        )
    );
}

#[test]
fn approval_hooks_behave() {
    let shell = ToolAction {
        display: None,
        command: Vec::new(),
        tool: "shell".to_string(),
        summary: "run echo".to_string(),
        risk: RiskLevel::Execute,
    };
    assert_eq!(AllowAllHook.approve(&shell), Approval::AllowAlways);
    assert_eq!(DenyAllHook.approve(&shell), Approval::Deny);
    let hook = FnHook::new(|action: &ToolAction| {
        if action.tool == "git" {
            Approval::AllowOnce
        } else {
            Approval::Deny
        }
    });
    assert_eq!(hook.approve(&shell), Approval::Deny);
    let git = ToolAction {
        display: None,
        command: Vec::new(),
        tool: "git".to_string(),
        summary: "status".to_string(),
        risk: RiskLevel::Read,
    };
    assert_eq!(hook.approve(&git), Approval::AllowOnce);
}

#[test]
fn traversal_is_rejected() {
    let (tmp, root) = rooted_case();
    fs::write(tmp.path().join("outside.txt"), "secret").unwrap();
    let err = resolve_within_root(&root, "../outside.txt").unwrap_err();
    assert!(err.to_string().contains("escapes"), "{err}");
}

#[tokio::test]
async fn toolkit_read_rejects_traversal() {
    let (tmp, root) = rooted_case();
    fs::write(tmp.path().join("outside.txt"), "secret").unwrap();
    let tool = ReadFile::new_with_root_dir(root.to_string_lossy().into_owned());
    let err = tool
        .execute(json!({ "file_path": "../outside.txt" }))
        .await
        .unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[test]
fn absolute_escape_is_rejected() {
    let (tmp, root) = rooted_case();
    let outside = tmp.path().join("outside.txt");
    fs::write(&outside, "secret").unwrap();
    let err = resolve_within_root(&root, outside.to_str().unwrap()).unwrap_err();
    assert!(err.to_string().contains("escapes"), "{err}");
}

#[test]
fn absolute_path_inside_root_is_allowed() {
    let (_tmp, root) = rooted_case();
    let inside = root.join("a.txt");
    fs::write(&inside, "x").unwrap();
    let resolved = resolve_within_root(&root, inside.to_str().unwrap()).unwrap();
    assert!(resolved.ends_with("a.txt"), "{}", resolved.display());
}

#[cfg(unix)]
#[test]
fn symlink_escape_is_rejected() {
    let (tmp, root) = rooted_case();
    let outside = tmp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret.txt"), "secret").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    let err = resolve_within_root(&root, "link/secret.txt").unwrap_err();
    assert!(err.to_string().contains("escapes"), "{err}");
}

#[cfg(unix)]
#[tokio::test]
async fn toolkit_read_rejects_symlink_escape() {
    let (tmp, root) = rooted_case();
    let outside = tmp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret.txt"), "secret").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    let tool = ReadFile::new_with_root_dir(root.to_string_lossy().into_owned());
    let err = tool
        .execute(json!({ "file_path": "link/secret.txt" }))
        .await
        .unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[tokio::test]
async fn write_file_inside_root_works() {
    let (_tmp, root) = rooted_case();
    let tool = WriteFile::new_with_root_dir(root.to_string_lossy().into_owned());
    let result = tool
        .execute(json!({
            "file_path": "sub/note.txt",
            "content": "hello",
            "append": false,
        }))
        .await
        .unwrap();
    assert_eq!(result["success"].as_bool(), Some(true));
    assert_eq!(
        fs::read_to_string(root.join("sub/note.txt")).unwrap(),
        "hello"
    );
}

#[tokio::test]
async fn shell_denied_under_deny_all() {
    let (_tmp, root) = rooted_case();
    let tool = ShellTool::new(root, deny_all());
    let err = tool
        .execute(json!({ "command": "echo", "args": ["hi"] }))
        .await
        .unwrap_err();
    let message = err.to_string().to_lowercase();
    assert!(message.contains("approv"), "{message}");
}

#[tokio::test]
async fn shell_echo_works_under_allow_all() {
    let (_tmp, root) = rooted_case();
    let tool = ShellTool::new(root, allow_all());
    let result = tool
        .execute(json!({ "command": "echo", "args": ["hello-tools"] }))
        .await
        .unwrap();
    assert_eq!(result["success"].as_bool(), Some(true));
    assert!(result["output"].as_str().unwrap().contains("hello-tools"));
}

#[tokio::test]
async fn shell_rejects_cwd_outside_root() {
    let (tmp, root) = rooted_case();
    let tool = ShellTool::new(root.clone(), allow_all());
    let err = tool
        .execute(json!({ "command": "echo", "args": ["hi"], "cwd": ".." }))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("escapes"), "{err}");
    let err = tool
        .execute(json!({
            "command": "echo",
            "args": ["hi"],
            "cwd": tmp.path().to_str().unwrap(),
        }))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("escapes"), "{err}");

    // Positive control: a cwd inside root works.
    fs::create_dir(root.join("sub")).unwrap();
    let result = tool
        .execute(json!({ "command": "echo", "args": ["hi"], "cwd": "sub" }))
        .await
        .unwrap();
    assert_eq!(result["success"].as_bool(), Some(true));
}

#[tokio::test]
async fn apply_patch_happy_path() {
    let (_tmp, root) = rooted_case();
    fs::write(root.join("hello.txt"), "line1\nline2\n").unwrap();
    let tool = ApplyPatchTool::new(root.clone(), allow_all());

    let modify = "--- a/hello.txt\n+++ b/hello.txt\n@@ -1,2 +1,3 @@\n line1\n line2\n+line3\n";
    let result = tool.execute(json!({ "patch": modify })).await.unwrap();
    assert_eq!(result["success"].as_bool(), Some(true));
    assert_eq!(
        fs::read_to_string(root.join("hello.txt")).unwrap(),
        "line1\nline2\nline3\n"
    );

    let create = "--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n";
    tool.execute(json!({ "patch": create })).await.unwrap();
    assert_eq!(
        fs::read_to_string(root.join("new.txt")).unwrap(),
        "one\ntwo\n"
    );
}

#[tokio::test]
async fn unrestricted_apply_patch_reports_project_relative_paths() {
    // YOLO plans against `/`; the result read `private/tmp/.../src/main.js`.
    let (_tmp, root) = rooted_case();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.js"), "a\n").unwrap();
    let tool = ApplyPatchTool::new(root.clone(), allow_all()).with_unrestricted_paths(true);
    let absolute = root.join("src/main.js");
    for path in ["src/main.js", absolute.to_str().unwrap()] {
        let patch = format!("--- a/{path}\n+++ b/{path}\n@@ -1 +1,2 @@\n a\n+b\n");
        let result = tool.execute(json!({ "patch": patch })).await.unwrap();
        assert_eq!(result["files"], json!(["src/main.js"]), "{path}");
        fs::write(&absolute, "a\n").unwrap();
    }
}

#[tokio::test]
async fn apply_patch_forgives_the_mistakes_models_make_but_not_ambiguity() {
    let (_tmp, root) = rooted_case();
    let tool = ApplyPatchTool::new(root.clone(), allow_all());
    let settings =
        "\"\"\"Settings.\"\"\"\n\nHOST = \"localhost\"\nTIMEOUT = 30  # seconds\nRETRIES = 3\n";
    fs::write(root.join("s.py"), settings).unwrap();
    // Miscounted header, wrong start line and single-spaced context: still one clear place.
    let sloppy = "--- a/s.py\n+++ b/s.py\n@@ -9,7 +9,7 @@\n HOST = \"localhost\"\n-TIMEOUT = 30 # seconds\n+TIMEOUT = 45  # seconds\n";
    tool.execute(json!({ "patch": sloppy })).await.unwrap();
    let text = fs::read_to_string(root.join("s.py")).unwrap();
    assert!(
        text.contains("TIMEOUT = 45  # seconds") && text.contains("RETRIES = 3"),
        "{text}"
    );
    assert!(text.starts_with("\"\"\"Settings."), "{text}");

    // The exact match nearest the declared line wins over an earlier duplicate.
    fs::write(root.join("d.txt"), "x\nsame\ny\nsame\nz\n").unwrap();
    let near = "--- a/d.txt\n+++ b/d.txt\n@@ -4,1 +4,1 @@\n-same\n+second\n";
    tool.execute(json!({ "patch": near })).await.unwrap();
    assert_eq!(
        fs::read_to_string(root.join("d.txt")).unwrap(),
        "x\nsame\ny\nsecond\nz\n"
    );

    // Two places that match only loosely: refused, nothing changed, file lines shown.
    fs::write(root.join("a.txt"), "a  b\nc\na b\n").unwrap();
    let ambiguous = "--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-a   b\n+changed\n";
    let error = tool
        .execute(json!({ "patch": ambiguous }))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("ambiguous context")
            && error.contains("match 2 places")
            && error.contains("starting at lines 1, 3")
            && error.contains("   1| a  b")
            && error.contains("   3| a b"),
        "{error}"
    );
    assert_eq!(
        fs::read_to_string(root.join("a.txt")).unwrap(),
        "a  b\nc\na b\n"
    );

    // A tab-indented context line missing its leading space is still matched as context.
    fs::write(
        root.join("Makefile"),
        "build:\n\tcargo build\n\ntest:\n\tcargo test  \n",
    )
    .unwrap();
    let tabbed = "--- a/Makefile\n+++ b/Makefile\n@@ -1,5 +1,5 @@\n build:\n-\tcargo build\n+\tcargo build --locked\n \n test:\n\tcargo test  \n";
    tool.execute(json!({ "patch": tabbed })).await.unwrap();
    assert_eq!(
        fs::read_to_string(root.join("Makefile")).unwrap(),
        "build:\n\tcargo build --locked\n\ntest:\n\tcargo test  \n"
    );

    // Text that is not in the file at all is still refused.
    let missing = "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-nowhere\n+x\n";
    assert!(tool.execute(json!({ "patch": missing })).await.is_err());
}

#[tokio::test]
async fn apply_patch_rejects_unsupported_syntax_without_claiming_success() {
    let (_tmp, root) = rooted_case();
    fs::write(root.join("file.txt"), "old\n").unwrap();
    let tool = ApplyPatchTool::new(root.clone(), allow_all());
    let result = tool.execute(json!({"patch":"*** Begin Patch\n*** Update File: file.txt\n@@\n-old\n+new\n*** End Patch\n"})).await;
    assert!(
        result.is_err(),
        "unsupported patch must not report an empty successful edit"
    );
    assert_eq!(fs::read_to_string(root.join("file.txt")).unwrap(), "old\n");
}

#[tokio::test]
async fn a_mistyped_shell_argument_is_named_in_the_error() {
    let (_tmp, root) = rooted_case();
    let tool = ShellTool::new(root, allow_all());
    let error = tool
        .execute(json!({"command": "sh", "args": ["-c", "true"], "cwd": {}}))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("argument 'cwd' is invalid"), "{error}");
    assert!(error.contains("expected a string"), "{error}");
    let error = tool
        .execute(json!({"command": 3}))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("argument 'command' is invalid"), "{error}");
}

#[tokio::test]
async fn apply_patch_ignores_a_stray_end_marker_after_a_unified_diff() {
    let (_tmp, root) = rooted_case();
    fs::write(root.join("file.txt"), "keep\nold\n").unwrap();
    let tool = ApplyPatchTool::new(root.clone(), allow_all());
    let patch =
        "--- a/file.txt\n+++ b/file.txt\n@@ -1,2 +1,2 @@\n keep\n-old\n+new\n*** End Patch\n";
    tool.execute(json!({ "patch": patch })).await.unwrap();
    assert_eq!(
        fs::read_to_string(root.join("file.txt")).unwrap(),
        "keep\nnew\n"
    );
}

#[test]
fn apply_patch_rejects_unprefixed_empty_hunk_lines() {
    let error = match parse_hunk(&["@@ -1 +1 @@", ""], 0) {
        Ok(_) => panic!("unprefixed empty hunk line was accepted"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("unexpected hunk line"));
}

#[tokio::test]
async fn apply_patch_rejects_out_of_root_hunk() {
    let (tmp, root) = rooted_case();
    let tool = ApplyPatchTool::new(root, allow_all());
    let evil = "--- /dev/null\n+++ b/../../evil.txt\n@@ -0,0 +1 @@\n+pwned\n";
    let err = tool.execute(json!({ "patch": evil })).await.unwrap_err();
    assert!(err.to_string().contains("escapes"), "{err}");
    assert!(!tmp.path().join("evil.txt").exists());
    assert!(!tmp.path().join("root/evil.txt").exists());
}

#[tokio::test]
async fn apply_patch_requires_approval() {
    let (_tmp, root) = rooted_case();
    fs::write(root.join("f.txt"), "a\n").unwrap();
    let tool = ApplyPatchTool::new(root, deny_all());
    let patch = "--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n-a\n+b\n";
    let err = tool.execute(json!({ "patch": patch })).await.unwrap_err();
    let message = err.to_string().to_lowercase();
    assert!(message.contains("approv"), "{message}");
}

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn git_status_works_in_temp_repo() {
    if !git_available() {
        eprintln!("skipping git_status_works_in_temp_repo: git binary not found");
        return;
    }
    let (_tmp, root) = rooted_case();
    let inited = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(inited.success());
    let tool = GitTool::new(root, allow_all());
    let result = tool
        .execute(json!({ "args": ["status", "--short"] }))
        .await
        .unwrap();
    assert_eq!(result["success"].as_bool(), Some(true));
}

#[tokio::test]
async fn git_allowlist_denies_unknown_subcommands() {
    let (_tmp, root) = rooted_case();
    let tool = GitTool::new(root, allow_all());
    let err = tool
        .execute(json!({ "args": ["rebase", "main"] }))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("denied"), "{err}");
}

#[tokio::test]
async fn git_tool_reports_argv_and_escalates_forced_variants() {
    let (_tmp, root) = rooted_case();
    let hook = Arc::new(RecordingHook::new(Approval::Deny));
    let tool = GitTool::new(root, hook.clone());
    for args in [
        vec!["status"],
        vec!["checkout", "main"],
        vec!["checkout", "-f", "main"],
        vec!["branch", "-D", "x"],
        vec!["checkout", "--", "."],
        vec!["commit", "--amend", "--no-edit"],
        vec!["push", "origin", "main"],
        vec!["push", "--force"],
        vec!["fetch"],
        vec!["pull", "--rebase"],
        vec!["merge", "feature"],
        vec!["merge", "--abort"],
    ] {
        let _ = tool.execute(json!({ "args": args })).await;
    }
    // Denied by the allowlist before any approval is consulted.
    assert!(tool
        .execute(json!({"args": ["reset", "--hard"]}))
        .await
        .is_err());
    let seen = hook.seen();
    assert_eq!(seen.len(), 12);
    let risks: Vec<RiskLevel> = seen[6..].iter().map(|action| action.risk).collect();
    assert_eq!(
        risks,
        [
            RiskLevel::Network,
            RiskLevel::Destructive,
            RiskLevel::Network,
            RiskLevel::Destructive,
            RiskLevel::Write,
            RiskLevel::Destructive
        ]
    );
    assert_eq!(seen[4].risk, RiskLevel::Destructive);
    assert_eq!(seen[5].risk, RiskLevel::Destructive);
    assert_eq!(seen[0].risk, RiskLevel::Read);
    assert_eq!(seen[1].risk, RiskLevel::Write);
    assert_eq!(seen[2].risk, RiskLevel::Destructive);
    assert_eq!(seen[2].command, vec!["git", "checkout", "-f", "main"]);
    assert_eq!(seen[3].risk, RiskLevel::Destructive);
    assert_ne!(seen[1].grant_key(), seen[2].grant_key());
}

#[tokio::test]
async fn git_write_requires_approval() {
    // No git binary needed: the denial happens before any spawn.
    let (_tmp, root) = rooted_case();
    let tool = GitTool::new(root, deny_all());
    let err = tool
        .execute(json!({ "args": ["add", "."] }))
        .await
        .unwrap_err();
    let message = err.to_string().to_lowercase();
    assert!(message.contains("approv"), "{message}");
}

#[test]
fn boxed_tools_returns_full_set() {
    let (_tmp, root) = rooted_case();
    let tools = boxed_tools(&root, allow_all()).unwrap();
    let names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
    assert_eq!(
        names,
        vec![
            "list_dir",
            "read_file",
            "write_file",
            "copy_file",
            "move_file",
            "delete_file",
            "create_dir",
            "search_file",
            "shell",
            "background_task",
            "apply_patch",
            "git",
        ]
    );
    for tool in &tools {
        assert!(!tool.description().is_empty());
        assert!(tool.args_schema().is_object());
    }
}

#[test]
fn boxed_tools_rejects_missing_root() {
    let err = boxed_tools(Path::new("/definitely/not/here/themis-test"), allow_all()).unwrap_err();
    assert!(err.to_string().contains("not accessible"), "{err}");
}

#[test]
fn shared_tools_convert_to_boxes() {
    let (_tmp, root) = rooted_case();
    let shared = shared_tools(&root, allow_all()).unwrap();
    assert_eq!(shared.len(), 12);
    let boxes = autoagents::core::tool::shared_tools_to_boxes(&shared);
    assert_eq!(boxes.len(), 12);
    assert_eq!(boxes[8].name(), "shell");
}
