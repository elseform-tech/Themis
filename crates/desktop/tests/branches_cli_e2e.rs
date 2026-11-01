//! Another branch's diff and switching branches, through the real server and `themis` CLI.

use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};
use themis_desktop::{server::Server, state::AppState, types::ProviderKind};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(root)
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?}");
}

fn call(data: &Path, command: &str, args: Value) -> Result<Value, String> {
    let output = Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(["call", command, &args.to_string()])
        .env_remove("OPENCODE_KEY")
        .output()
        .expect("CLI");
    if output.status.success() {
        Ok(serde_json::from_slice(&output.stdout).unwrap_or(Value::Null))
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

/// `main` with `shared.txt`, and `feature` adding `new.txt` and changing `shared.txt`.
fn repository(root: &Path) {
    git(root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("shared.txt"), "one\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "first"]);
    git(root, &["switch", "-qc", "feature"]);
    std::fs::write(root.join("shared.txt"), "two\n").unwrap();
    std::fs::write(root.join("new.txt"), "added\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "second"]);
    git(root, &["switch", "-q", "main"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_branch_is_read_without_switching_and_switching_guards_uncommitted_work() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    repository(project.path());
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(async move {
        let _ = server.run().await;
    });
    let (dir, root) = (data.path().to_owned(), project.path().to_owned());
    tokio::task::spawn_blocking(move || {
        let root_arg = root.to_string_lossy().to_string();
        let current = |root: &Path| {
            String::from_utf8(
                Command::new("git")
                    .args(["branch", "--show-current"])
                    .current_dir(root)
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
            .trim()
            .to_owned()
        };
        // Reading another branch's changes leaves the checkout alone.
        let diff = call(
            &dir,
            "branch_diff",
            json!({"projectRoot": root_arg, "branch": "feature"}),
        )
        .unwrap();
        let paths: Vec<_> = diff["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].clone())
            .collect();
        assert_eq!(paths, vec![json!("new.txt"), json!("shared.txt")]);
        assert_eq!(current(&root), "main");
        // Only local branches: not options, ranges or other references.
        for bad in ["--output=/tmp/x", "main..feature", "HEAD", "missing"] {
            let error = call(
                &dir,
                "branch_diff",
                json!({"projectRoot": root_arg, "branch": bad}),
            )
            .unwrap_err();
            assert!(error.contains("Unknown branch"), "{bad}: {error}");
            assert!(call(
                &dir,
                "switch_branch",
                json!({"projectRoot": root_arg, "branch": bad})
            )
            .is_err());
        }
        // A switch that would overwrite uncommitted work is refused, and says why in one line.
        std::fs::write(root.join("shared.txt"), "mine\n").unwrap();
        let error = call(
            &dir,
            "switch_branch",
            json!({"projectRoot": root_arg, "branch": "feature"}),
        )
        .unwrap_err();
        assert!(error.contains("overwritten"), "{error}");
        assert_eq!(current(&root), "main");
        assert_eq!(
            std::fs::read_to_string(root.join("shared.txt")).unwrap(),
            "mine\n"
        );
        // Clean again: it switches, and reports the new branch.
        git(&root, &["checkout", "--", "shared.txt"]);
        let info = call(
            &dir,
            "switch_branch",
            json!({"projectRoot": root_arg, "branch": "feature"}),
        )
        .unwrap();
        assert_eq!(info["branch"], "feature");
        assert_eq!(current(&root), "feature");
    })
    .await
    .unwrap();
    serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_waits_while_a_chat_is_working_in_that_checkout() {
    let llm = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_secs(3))
                .set_body_json(json!({
                    "choices": [{"message": {"role": "assistant", "content": "Done."}, "finish_reason": "stop"}]
                })),
        )
        .mount(&llm)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    repository(project.path());
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(llm.uri()));
    let thread = state
        .create_thread(
            project.path().to_string_lossy().into(),
            ProviderKind::Go,
            Some("test-model".into()),
        )
        .await
        .unwrap()
        .id;
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(async move {
        let _ = server.run().await;
    });
    let (dir, root) = (data.path().to_owned(), project.path().to_owned());
    tokio::task::spawn_blocking(move || {
        call(
            &dir,
            "send_message",
            json!({"threadId": thread, "text": "hello"}),
        )
        .unwrap();
        let root_arg = root.to_string_lossy().to_string();
        let error = call(
            &dir,
            "switch_branch",
            json!({"projectRoot": root_arg, "branch": "feature"}),
        )
        .unwrap_err();
        assert!(error.contains("working in this project"), "{error}");
    })
    .await
    .unwrap();
    serving.abort();
}

/// The composer's switch runs outside the sandbox: a hook the repository names doesn't start.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn switching_starts_none_of_the_repositorys_hooks() {
    use std::os::unix::fs::PermissionsExt;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    repository(project.path());
    let marker = project.path().join("hook-ran");
    let hook = project.path().join(".git/hooks/post-checkout");
    std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(async move {
        let _ = server.run().await;
    });
    let (dir, root) = (data.path().to_owned(), project.path().to_owned());
    let done = tokio::task::spawn_blocking(move || {
        call(
            &dir,
            "switch_branch",
            json!({"projectRoot": root.to_string_lossy(), "branch": "feature"}),
        )
    })
    .await
    .unwrap();
    serving.abort();
    assert_eq!(done.unwrap()["branch"], "feature");
    assert!(!marker.exists(), "the repository's hook ran");
}
