//! Read-only workspace monitor commands through the shared app server and CLI.
use serde_json::{json, Value};
use std::path::Path;
use themis_desktop::{
    server::{Client, Server},
    state::AppState,
};

fn cli(dir: &Path, command: &str, args: Value) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir)
        .args(["call", command, &args.to_string()])
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_workspace_reports_branches_lists_files_and_stays_inside_the_project() {
    let data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let project = parent.path().join("repo");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("src/a.txt"), "alpha\n").unwrap();
    std::fs::write(parent.path().join("outside.txt"), "secret").unwrap();
    git(&project, &["init", "-q", "-b", "main"]);
    git(
        &project,
        &["-c", "user.email=t@t", "-c", "user.name=t", "add", "."],
    );
    git(
        &project,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "init",
        ],
    );
    git(&project, &["branch", "feature/monitor"]);
    let head = std::fs::read(project.join(".git/HEAD")).unwrap();

    let state = AppState::new_for_test(data.path().join("settings.json"));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let root = cli(data.path(), "open_project", json!({"path": project}))["root"].clone();

    let workspace = cli(data.path(), "get_workspace", json!({"projectRoot": root}));
    assert_eq!(workspace["is_git"], true);
    assert_eq!(workspace["branch"], "main");
    let branches: Vec<&str> = workspace["branches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|branch| branch["name"].as_str().unwrap())
        .collect();
    assert!(branches.contains(&"main") && branches.contains(&"feature/monitor"));
    assert_eq!(
        workspace["pull_requests"],
        json!([]),
        "pull requests are fetched only on request"
    );

    let listing = cli(
        data.path(),
        "list_project_dir",
        json!({"projectRoot": root, "path": ""}),
    );
    let names: Vec<&str> = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["src"], "folders first and .git hidden");

    let file = cli(
        data.path(),
        "read_project_file",
        json!({"projectRoot": root, "path": "src/a.txt"}),
    );
    assert_eq!(file["content"], "alpha\n");
    assert_eq!(file["binary"], false);

    let client = Client::new(data.path().into());
    for path in ["../outside.txt", "/etc/hosts"] {
        let error = client
            .call(
                "read_project_file",
                json!({"projectRoot": root, "path": path}),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("invalid path"), "{error}");
    }
    assert_eq!(
        std::fs::read(project.join(".git/HEAD")).unwrap(),
        head,
        "monitoring never changes the checkout"
    );
    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_change_list_shows_only_readable_files_and_counts_hidden_ones() {
    let data = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let project = cli(data.path(), "create_project", json!({"name": "Monitor"}));
    let root = std::path::PathBuf::from(project["root"].as_str().unwrap());
    std::fs::create_dir_all(root.join("__pycache__")).unwrap();
    std::fs::write(root.join("app.py"), "print('hi')\n").unwrap();
    std::fs::write(root.join("__pycache__/app.cpython-314.pyc"), "bytecode").unwrap();
    std::fs::write(root.join("chart.png"), [0x89, b'P', b'N', b'G', 0, 0]).unwrap();
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot": root, "provider": "go", "model": "muse-spark-1.3-contributor"}),
    );

    let diff = cli(data.path(), "list_diff", json!({"threadId": thread["id"]}));
    let paths: Vec<&str> = diff["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["app.py"]);
    assert_eq!(diff["reason"], "2 binary or generated files hidden");

    Client::new(data.path().into())
        .call("shutdown", json!({}))
        .await
        .unwrap();
    serving.await.unwrap().unwrap();
}
