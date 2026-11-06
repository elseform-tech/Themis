use super::*;
use crate::sandbox::{Availability, Enforcement, PlanInputs, SandboxConfig, SandboxPlan};

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    outside: PathBuf,
    app: PathBuf,
    home: PathBuf,
}

/// Folders outside the temporary folder (which stays writable).
fn env() -> Env {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/sandbox-tests");
    fs::create_dir_all(&base).unwrap();
    let dir = tempfile::tempdir_in(&base).unwrap();
    let make = |name: &str| {
        let path = dir.path().join(name);
        fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    };
    let (root, outside, app, home) = (make("root"), make("outside"), make("app"), make("home"));
    fs::create_dir_all(home.join(".ssh")).unwrap();
    fs::write(home.join(".ssh/id"), "secret").unwrap();
    fs::write(app.join("runtime.jsonc"), "{}").unwrap();
    Env {
        _dir: dir,
        root,
        outside,
        app,
        home,
    }
}

fn plan(env: &Env, json: &str) -> Arc<SandboxPlan> {
    let config: SandboxConfig = serde_json::from_str(json).unwrap();
    Arc::new(SandboxPlan::new(PlanInputs {
        config: &config,
        project_root: &env.root,
        app_data: Some(&env.app),
        home: Some(&env.home),
        open: None,
    }))
}

fn active(env: &Env) -> Option<Arc<SandboxPlan>> {
    let plan = plan(env, "{}");
    if plan.is_active() {
        Some(plan)
    } else {
        eprintln!("skipping sandboxed tool test: {:?}", plan.enforcement());
        None
    }
}

fn sh(script: &str) -> Value {
    json!({"command": "sh", "args": ["-c", script]})
}

#[tokio::test]
async fn yolo_reports_a_block_to_the_agent_without_asking() {
    let env = env();
    let Some(plan) = active(&env) else { return };
    let hook = Arc::new(RecordingHook::new(Approval::AllowOnce));
    let tool = ShellTool::new(env.root.clone(), hook.clone())
        .with_unrestricted_paths(true)
        .with_sandbox(plan, false);
    let inside = tool.execute(sh("echo ok > inside.txt")).await.unwrap();
    assert_eq!(inside["success"], true, "{inside}");
    assert!(env.root.join("inside.txt").exists());
    let target = env.outside.join("x.txt");
    let result = tool
        .execute(sh(&format!("echo x > '{}'", target.display())))
        .await
        .unwrap();
    assert_eq!(result["success"], false);
    assert!(!target.exists());
    let output = result["output"].as_str().unwrap();
    assert!(
        output.contains("Themis sandbox blocked this command"),
        "{output}"
    );
    assert!(result["sandbox_blocked"].is_string(), "{result}");
    // Only the two shell calls were checked; nothing asked to escalate.
    assert!(hook.seen().iter().all(|action| action.tool == "shell"));
}

#[tokio::test]
async fn custom_asks_to_run_outside_and_allow_once_reruns_unsandboxed() {
    let env = env();
    let Some(plan) = active(&env) else { return };
    let hook = Arc::new(RecordingHook::new(Approval::AllowOnce));
    let tool = ShellTool::new(env.root.clone(), hook.clone()).with_sandbox(plan, true);
    let target = env.outside.join("escalated.txt");
    let result = tool
        .execute(sh(&format!("echo x > '{}'", target.display())))
        .await
        .unwrap();
    assert_eq!(result["success"], true, "{result}");
    assert_eq!(result["ran_outside_sandbox"], true);
    assert!(target.exists());
    let seen = hook.seen();
    let escalation = seen
        .iter()
        .find(|a| a.tool == UNSANDBOXED_TOOL)
        .expect("escalation asked");
    assert_eq!(escalation.command[0], "sh");
    let blocked = escalation
        .display
        .as_ref()
        .unwrap()
        .sandbox_blocked
        .clone()
        .unwrap();
    assert!(blocked.contains("escalated.txt"), "{blocked}");
    assert!(escalation.grantable());
    assert_eq!(
        escalation.grant_scope().unwrap().argv,
        escalation.command,
        "grant keyed like other shell commands"
    );
}

#[tokio::test]
async fn a_denied_escalation_keeps_the_failure_and_says_so() {
    let env = env();
    let Some(plan) = active(&env) else { return };
    let hook = Arc::new(FnHook::new(|action: &ToolAction| {
        if action.tool == UNSANDBOXED_TOOL {
            Approval::Deny
        } else {
            Approval::AllowOnce
        }
    }));
    let tool = ShellTool::new(env.root.clone(), hook).with_sandbox(plan, true);
    let target = env.outside.join("denied.txt");
    let result = tool
        .execute(sh(&format!("echo x > '{}'", target.display())))
        .await
        .unwrap();
    assert_eq!(result["success"], false);
    assert!(!target.exists());
    assert!(result["output"]
        .as_str()
        .unwrap()
        .contains("was not approved"));
}

#[tokio::test]
async fn destructive_escalations_cannot_be_granted_for_the_chat() {
    let env = env();
    let Some(plan) = active(&env) else { return };
    let victim = env.outside.join("tree");
    fs::create_dir_all(victim.join("sub")).unwrap();
    let hook = Arc::new(RecordingHook::new(Approval::Deny));
    let tool = ShellTool::new(
        env.root.clone(),
        Arc::new(FnHook::new({
            let hook = hook.clone();
            move |action: &ToolAction| {
                hook.approve(action);
                if action.tool == UNSANDBOXED_TOOL {
                    Approval::Deny
                } else {
                    Approval::AllowOnce
                }
            }
        })),
    )
    .with_sandbox(plan, true);
    let result = tool
        .execute(json!({"command": "rm", "args": ["-rf", victim.to_string_lossy()]}))
        .await
        .unwrap();
    assert_eq!(result["success"], false, "{result}");
    assert!(victim.exists());
    let escalation = hook
        .seen()
        .into_iter()
        .find(|a| a.tool == UNSANDBOXED_TOOL)
        .expect("asked");
    assert_eq!(escalation.risk, RiskLevel::Destructive);
    assert!(!escalation.grantable());
}

#[tokio::test]
async fn secrets_and_app_data_stay_out_of_reach() {
    let env = env();
    let Some(plan) = active(&env) else { return };
    let tool = ShellTool::new(env.root.clone(), allow_all())
        .with_unrestricted_paths(true)
        .with_sandbox(plan, false);
    let read = tool
        .execute(sh(&format!("cat '{}'", env.home.join(".ssh/id").display())))
        .await
        .unwrap();
    assert_eq!(read["success"], false);
    assert!(!read["output"].as_str().unwrap().contains("secret\n"));
    let config = env.app.join("runtime.jsonc");
    let write = tool
        .execute(sh(&format!(
            "echo '{{\"approval\":{{}}}}' > '{}'",
            config.display()
        )))
        .await
        .unwrap();
    assert_eq!(write["success"], false);
    assert_eq!(fs::read_to_string(&config).unwrap(), "{}");
}

#[tokio::test]
async fn git_tool_runs_inside_the_sandbox() {
    if !std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        return;
    }
    let env = env();
    let Some(plan) = active(&env) else { return };
    let status = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&env.root)
        .status()
        .unwrap();
    assert!(status.success());
    let git = GitTool::new(env.root.clone(), allow_all())
        .with_unrestricted_commands(true)
        .with_sandbox(plan, false);
    let ok = git
        .execute(json!({"args": ["status", "--short"]}))
        .await
        .unwrap();
    assert_eq!(ok["success"], true, "{ok}");
    let target = env.outside.join("gitconfig");
    let blocked = git
        .execute(json!({"args": ["config", "--file", target.to_string_lossy(), "user.name", "x"]}))
        .await
        .unwrap();
    assert_eq!(blocked["success"], false, "{blocked}");
    assert!(!target.exists());
}

#[tokio::test]
async fn background_tasks_run_inside_the_sandbox() {
    let env = env();
    let Some(plan) = active(&env) else { return };
    let registry = background::BackgroundRegistry::in_memory();
    let tool = ShellTool::new(env.root.clone(), allow_all())
        .with_unrestricted_paths(true)
        .with_background(background::BackgroundScope::new(
            Arc::clone(&registry),
            "t1",
        ))
        .with_sandbox(plan, true);
    let target = env.outside.join("bg.txt");
    let started = tool
                .execute(json!({"command": "sh", "args": ["-c", format!("echo x > '{}'; echo done", target.display())], "background": true}))
                .await
                .unwrap();
    assert_eq!(started["sandboxed"], true, "{started}");
    let id = started["task_id"].as_str().unwrap().to_owned();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let view = registry.status("t1", &id).unwrap();
        if view.record.state != background::TaskState::Running || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!target.exists(), "background task escaped the sandbox");
}

#[test]
fn a_read_only_call_escalates_as_execute_and_only_a_person_lets_it_out() {
    let env = env();
    let Some(plan) = active(&env) else { return };
    let outside = env.outside.join("x");
    let blocked = || CommandOutput {
        success: false,
        exit_code: Some(1),
        output: format!("error: {}: Operation not permitted", outside.display()),
        truncated: false,
    };
    let status = ToolAction {
        display: None,
        command: vec!["git".into(), "status".into()],
        tool: "git".into(),
        summary: "run `git status`".into(),
        risk: RiskLevel::Read,
    };
    let run = |policy: &str, answer: Approval, unattended: bool| {
        let asked = Arc::new(std::sync::Mutex::new(Vec::<ToolAction>::new()));
        let interactive = Arc::new(FnHook::new({
            let asked = asked.clone();
            move |action: &ToolAction| {
                asked.lock().unwrap().push(action.clone());
                if unattended {
                    Approval::Deny
                } else {
                    answer
                }
            }
        }));
        let policy = crate::configuration::resolve(Some(policy), None, None)
            .unwrap()
            .config
            .approval;
        let hook = crate::configuration::configured_hook(
            crate::configuration::ApprovalMode::Custom,
            policy,
            interactive,
        );
        let outcome = sandbox_outcome(
            blocked(),
            Some(&plan),
            true,
            hook.as_ref(),
            &status,
            &env.root,
        );
        let asked = asked.lock().unwrap().clone();
        (matches!(outcome, SandboxOutcome::RunOutside), asked)
    };
    // The default policy allows reads, but the escape is an execution and asks.
    let (outside_run, asked) = run("{}", Approval::Deny, false);
    assert!(!outside_run);
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].tool, UNSANDBOXED_TOOL);
    assert_eq!(asked[0].risk, RiskLevel::Execute);
    for broad in [
        r#"{"approval":{"rules":[{"tool":"*","action":"allow"}]}}"#,
        r#"{"approval":{"rules":[{"tool":"shell*","action":"allow"}]}}"#,
        r#"{"approval":{"rules":[{"tool":"*","risk":"execute","action":"allow"}]}}"#,
    ] {
        let (outside_run, asked) = run(broad, Approval::Deny, false);
        assert!(!outside_run, "{broad}");
        assert_eq!(asked.len(), 1, "{broad}");
    }
    // Unattended (nobody answers): stays inside.
    assert!(!run("{}", Approval::AllowOnce, true).0);
    // A person's Allow once lets it out; a deny rule refuses without asking.
    assert!(run("{}", Approval::AllowOnce, false).0);
    let (outside_run, asked) = run(
        r#"{"approval":{"rules":[{"tool":"*","action":"deny"}]}}"#,
        Approval::AllowOnce,
        false,
    );
    assert!(!outside_run && asked.is_empty());
}

#[tokio::test]
async fn an_unusable_configured_backend_fails_closed() {
    let env = env();
    let backend = if cfg!(target_os = "macos") {
        "seatbelt"
    } else {
        "bubblewrap"
    };
    let config: SandboxConfig =
        serde_json::from_str(&format!(r#"{{"backend":"{backend}"}}"#)).unwrap();
    let plan = Arc::new(SandboxPlan::with_availability(
        PlanInputs {
            config: &config,
            project_root: &env.root,
            app_data: Some(&env.app),
            home: Some(&env.home),
            open: None,
        },
        |_| Availability::no("missing for this test"),
    ));
    assert!(matches!(plan.enforcement(), Enforcement::FailClosed(..)));
    let tool = ShellTool::new(env.root.clone(), allow_all()).with_sandbox(Arc::clone(&plan), true);
    let error = tool
        .execute(sh("echo hi > ran.txt"))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains(backend), "{error}");
    assert!(!env.root.join("ran.txt").exists());
    let git = GitTool::new(env.root.clone(), allow_all()).with_sandbox(Arc::clone(&plan), true);
    assert!(git.execute(json!({"args": ["status"]})).await.is_err());
    let check = run_check_command(
        vec!["true".into()],
        env.root.clone(),
        Duration::from_secs(5),
        Some(plan),
    )
    .await;
    assert!(check.is_err());
    // `auto` with nothing usable never blocks: commands run as before.
    let auto = Arc::new(SandboxPlan::with_availability(
        PlanInputs {
            config: &SandboxConfig::default(),
            project_root: &env.root,
            app_data: Some(&env.app),
            home: Some(&env.home),
            open: None,
        },
        |_| Availability::no("missing for this test"),
    ));
    let tool = ShellTool::new(env.root.clone(), allow_all()).with_sandbox(auto, true);
    assert_eq!(
        tool.execute(sh("echo hi > ran.txt")).await.unwrap()["success"],
        true
    );
    assert!(env.root.join("ran.txt").exists());
}

#[tokio::test]
async fn file_tools_never_touch_app_data_or_secrets() {
    let env = env();
    let unavailable = Arc::new(SandboxPlan::with_availability(
        PlanInputs {
            config: &SandboxConfig::default(),
            project_root: &env.root,
            app_data: Some(&env.app),
            home: Some(&env.home),
            open: None,
        },
        |_| Availability::no("not needed for in-process guards"),
    ));
    let tools = boxed_tools_for_run(
        &env.root,
        allow_all(),
        ToolOptions {
            yolo: true,
            sandbox: Some(unavailable),
            ..ToolOptions::default()
        },
    )
    .unwrap();
    let config = env.app.join("runtime.jsonc");
    let error = find_tool(&tools, "write_file")
                .execute(json!({"file_path": config.to_string_lossy(), "content": "{\"sandbox\":{\"enabled\":false}}", "append": false}))
                .await
                .unwrap_err()
                .to_string();
    assert!(error.contains("Themis settings"), "{error}");
    assert_eq!(fs::read_to_string(&config).unwrap(), "{}");
    let error = find_tool(&tools, "read_file")
        .execute(json!({"file_path": env.home.join(".ssh/id").to_string_lossy()}))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("secret"), "{error}");
    let project_config = env.root.join(".themis/config.jsonc");
    assert!(find_tool(&tools, "write_file")
        .execute(
            json!({"file_path": project_config.to_string_lossy(), "content": "{}", "append": false})
        )
        .await
        .is_err());
    let patch = format!(
        "--- /dev/null\n+++ {}\n@@ -0,0 +1 @@\n+x\n",
        env.app.join("new.json").display()
    );
    assert!(find_tool(&tools, "apply_patch")
        .execute(json!({"patch": patch}))
        .await
        .is_err());
    assert!(!env.app.join("new.json").exists());
    // Ordinary project files still work.
    find_tool(&tools, "write_file")
        .execute(json!({"file_path": "notes.txt", "content": "hi", "append": false}))
        .await
        .unwrap();
    assert!(env.root.join("notes.txt").exists());
}
