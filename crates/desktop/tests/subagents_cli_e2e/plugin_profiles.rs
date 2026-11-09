//! Subagent profiles from plugins: import, spawn, tool narrowing, write paths, models and malformed definitions.

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_plugins_subagent_imports_spawns_and_runs_with_its_instructions() {
    let app = app(
        None,
        spawning(
            json!([{"write_paths": [], "name": "Review", "brief": "THE-BRIEF: look at notes.txt", "profile": "reviewer"}]),
            |_| reply("Reviewed: all fine."),
        ),
    )
    .await;
    let parent = tempfile::tempdir().unwrap();
    let folder = plugin_folder(
        parent.path(),
        "review-kit",
        &[(
            "agents/reviewer.md",
            &agent_file("reviewer", "tools: Read, Grep, Glob\nmodel: inherit\n"),
        )],
    );
    let data = app.data.path().to_owned();
    let plugin = on_cli({
        let data = data.clone();
        move || import_plugin(&data, &folder)
    })
    .await;
    assert_eq!(profile_names(&plugin), ["reviewer"], "{plugin}");
    // The profile lives in the plugin's own entry of the one registry.
    let registry = std::fs::read_to_string(data.join("plugins/registry.json")).unwrap();
    let registry: Value = serde_json::from_str(&registry).unwrap();
    assert_eq!(
        registry["current"]["review-kit"]["spec"]["subagents"][0]["name"], "reviewer",
        "{registry}"
    );
    run_in_yolo(&app, "review the project").await;
    let all = bodies(&app.llm).await;
    let main = all.iter().find(|body| !is_subagent(body)).unwrap();
    // The model can find the profile: its name and a short description are on the tool.
    let spawn = spawn_tool(main);
    let description = spawn["description"].as_str().unwrap();
    assert!(description.contains("reviewer"), "{description}");
    assert!(description.contains("Reviews code for bugs."));
    assert!(
        spawn["parameters"]["properties"]["subagents"]["items"]["properties"]["profile"]
            .is_object(),
        "{spawn}"
    );
    // The child got the body before the brief, and only what its list names.
    let child = all.iter().find(|body| is_subagent(body)).expect("a child");
    let whole: String = messages(child).iter().map(text).collect();
    let body_at = whole.find(PROFILE_BODY).expect("the profile's body");
    let brief_at = whole.find("THE-BRIEF").expect("the brief");
    assert!(body_at < brief_at, "{whole}");
    // In the system message, with the access it really has stated after it.
    let system = text(&messages(child)[0]);
    assert!(system.starts_with(SUBAGENT), "{system}");
    let role_at = system.find(PROFILE_BODY).expect("in the system message");
    let access_at = system
        .find(themis_core::prompts::SUBAGENT_READ_ONLY_NOTE)
        .expect("the access note");
    assert!(role_at < access_at, "{system}");
    // A shell held to a list of commands is not told about `themis`.
    assert!(!system.contains(themis_core::prompts::SUBAGENT_INTEGRATIONS_NOTE));
    // Grep and Glob are the shell, held to commands that only read (`subagent_tools_cli_e2e`);
    // on Windows they are the listing and search tools, with no shell.
    let reviewer: &[&str] = if cfg!(windows) {
        &["read_file"]
    } else {
        &["read_file", "shell"]
    };
    assert_eq!(sorted_tools(child), with_reads(reviewer));
    // Its result reached the main agent, and the listing names the profile.
    assert!(all.iter().any(|body| mentions(body, "Reviewed: all fine.")));
    let listed = call(
        app.data.path(),
        "list_subagents",
        json!({"threadId": app.thread}),
    );
    assert_eq!(listed[0]["profile"], "reviewer", "{listed}");
    app.serving.abort();
}

/// A project holds no plugins: importing one (and its subagents) into it is refused with where
/// to put it instead, and nothing is written there or saved anywhere.
#[tokio::test(flavor = "multi_thread")]
async fn importing_a_plugins_subagents_into_a_project_is_refused() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let serving = tokio::spawn(Server::bind(state, data.path()).await.unwrap().run());
    let parent = tempfile::tempdir().unwrap();
    let folder = plugin_folder(
        parent.path(),
        "project-kit",
        &[(
            "agents/local-reviewer.md",
            &agent_file("local-reviewer", ""),
        )],
    );
    let (path, root) = (data.path().to_owned(), project.path().to_owned());
    let (ok, plugin, error) = on_cli(move || {
        themis(
            &path,
            &[
                "plugin",
                "import",
                folder.to_str().unwrap(),
                "--scope",
                "local",
                "--project",
                root.to_str().unwrap(),
            ],
        )
    })
    .await;
    assert!(!ok, "{plugin}");
    assert!(
        error.contains(themis_core::plugins::PROJECT_SCOPE_GONE),
        "{error}"
    );
    assert!(!project.path().join(".themis").exists());
    let path = data.path().to_owned();
    let (_, listed, _) = on_cli(move || themis(&path, &["plugin", "list"])).await;
    assert_eq!(listed, json!([]), "nothing was saved");
    serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_list_only_narrows() {
    let mcp = lookup_server().await;
    let app = app(
        None,
        spawning(
            json!([
                {"write_paths": [], "name": "Named", "brief": "names mcp", "profile": "names-mcp"},
                {"write_paths": [], "name": "Omits", "brief": "omits mcp", "profile": "omits-mcp"},
                {"write_paths": [], "name": "Scoped", "brief": "scoped shell", "profile": "scoped-shell"},
                {"write_paths": [], "name": "Denied", "brief": "denied shell", "profile": "no-shell"},
                {"write_paths": [], "name": "Plain", "brief": "no list", "profile": "no-list"}
            ]),
            |_| reply("Done looking."),
        ),
    )
    .await;
    let parent = tempfile::tempdir().unwrap();
    let names_mcp = agent_file(
        "names-mcp",
        "tools:\n  - Read\n  - Bash\n  - WebFetch\n  - mcp__docs__lookup\n",
    );
    let omits_mcp = agent_file("omits-mcp", "tools: [\"Read\"]\n");
    let scoped = agent_file("scoped-shell", "tools: Read, Bash(git diff:*)\n");
    let no_shell = agent_file("no-shell", "disallowedTools: Bash(rm:*)\n");
    let no_list = agent_file("no-list", "");
    let mcp_file = json!({"mcpServers": {"docs": {"type": "http", "url": mcp.uri()}}}).to_string();
    let folder = plugin_folder(
        parent.path(),
        "narrow-kit",
        &[
            ("agents/names-mcp.md", &names_mcp),
            ("agents/omits-mcp.md", &omits_mcp),
            ("agents/scoped-shell.md", &scoped),
            ("agents/no-shell.md", &no_shell),
            ("agents/no-list.md", &no_list),
            (".mcp.json", &mcp_file),
        ],
    );
    let data = app.data.path().to_owned();
    let report = on_cli(move || {
        let (ok, report, error) = themis(&data, &["plugin", "inspect", folder.to_str().unwrap()]);
        assert!(ok, "{error}");
        let (ok, _, error) = themis(
            &data,
            &[
                "plugin",
                "import",
                folder.to_str().unwrap(),
                "--allow-partial",
            ],
        );
        assert!(ok, "{error}");
        let (ok, _, error) = themis(&data, &["mcp", "enable", "narrow-kit", "docs"]);
        assert!(ok, "{error}");
        report
    })
    .await;
    // Every name here means something, so nothing is dropped.
    let reasons = report["spec"]["unsupported"].to_string();
    assert!(!reasons.contains("mcp__docs__lookup"), "{reasons}");
    assert!(!reasons.contains("Bash(git diff:*)"), "{reasons}");

    run_in_yolo(&app, "review the project").await;
    let all = bodies(&app.llm).await;
    let main = all.iter().find(|body| !is_subagent(body)).unwrap();
    assert!(
        tool_names(main).iter().any(|name| name.contains("lookup")),
        "the main agent has the MCP tool: {:?}",
        tool_names(main)
    );
    let tools_of = |brief: &str| {
        let child = all
            .iter()
            .find(|body| is_subagent(body) && mentions(body, brief))
            .unwrap_or_else(|| panic!("no child for {brief}"));
        // What MCP tools they get has its own tests (`subagent_mcp_cli_e2e`).
        let mut names = sorted_tools(child);
        names.retain(|name| !name.starts_with("mcp_"));
        names
    };
    // What the list names, and no more.
    assert_eq!(
        tools_of("names mcp"),
        with_reads(&["fetch_url", "read_file", "shell"])
    );
    assert_eq!(tools_of("omits mcp"), with_reads(&["read_file"]));
    // A scoped Bash(...) gives the shell, held to its commands (`subagent_tools_cli_e2e`).
    assert_eq!(
        tools_of("scoped shell"),
        with_reads(&["read_file", "shell"])
    );
    // A list that denies part of the shell loses the shell.
    assert_eq!(
        tools_of("denied shell"),
        with_reads(&["fetch_url", "read_file"])
    );
    // No list: a subagent's usual read-only tools, and the web (this test has no search address).
    assert_eq!(
        tools_of("no list"),
        with_reads(&["fetch_url", "read_file", "shell"])
    );
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_listing_write_still_needs_write_paths() {
    let child = |body: &Value| match tool_results(body) {
        0 => tools(&[(
            "w1",
            "write_file",
            json!({"file_path": if mentions(body, "with paths") { "out/made.txt" } else { "stray.txt" },
                "content": "written"}),
        )]),
        _ => reply("Finished writing."),
    };
    let app = app(
        None,
        spawning(
            json!([
                {"write_paths": [], "name": "Without", "brief": "no paths given", "profile": "writer"},
                {"name": "With", "brief": "with paths given", "profile": "writer", "write_paths": ["out/"]}
            ]),
            child,
        ),
    )
    .await;
    let parent = tempfile::tempdir().unwrap();
    let folder = plugin_folder(
        parent.path(),
        "write-kit",
        &[(
            "agents/writer.md",
            &agent_file("writer", "tools: Read, Write, Edit\n"),
        )],
    );
    let data = app.data.path().to_owned();
    on_cli(move || import_plugin(&data, &folder)).await;
    run_in_yolo(&app, "write something").await;
    let all = bodies(&app.llm).await;
    let without = all
        .iter()
        .find(|body| is_subagent(body) && mentions(body, "no paths given"))
        .expect("the read-only child");
    assert_eq!(sorted_tools(without), with_reads(&["read_file"]));
    let with = all
        .iter()
        .find(|body| is_subagent(body) && mentions(body, "with paths given"))
        .expect("the writing child");
    assert_eq!(
        sorted_tools(with),
        with_reads(&["apply_patch", "read_file", "write_file"])
    );
    let root = app.project.path();
    assert!(!root.join("stray.txt").exists(), "no write without paths");
    assert_eq!(
        std::fs::read_to_string(root.join("out/made.txt")).unwrap(),
        "written"
    );
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn write_paths_for_a_profile_without_write_tools_are_refused() {
    let app = app(
        None,
        spawning(
            json!([{"name": "Reader", "brief": "x", "profile": "reader", "write_paths": ["out/"]}]),
            |_| reply("never"),
        ),
    )
    .await;
    let parent = tempfile::tempdir().unwrap();
    let folder = plugin_folder(
        parent.path(),
        "read-kit",
        &[("agents/reader.md", &agent_file("reader", "tools: Read\n"))],
    );
    let data = app.data.path().to_owned();
    on_cli(move || import_plugin(&data, &folder)).await;
    run_in_yolo(&app, "go").await;
    let all = bodies(&app.llm).await;
    assert!(all.iter().all(|body| !is_subagent(body)), "nothing started");
    let result = spawn_result(&all);
    assert!(result.contains("reader"), "{result}");
    assert!(result.contains("write_paths"), "{result}");
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_profile_fails_clearly_and_starts_nothing() {
    let app = app(
        None,
        spawning(
            json!([
                {"write_paths": [], "name": "Good", "brief": "fine", "profile": "reviewer"},
                {"write_paths": [], "name": "Bad", "brief": "broken", "profile": "no-such-profile"}
            ]),
            |_| reply("never"),
        ),
    )
    .await;
    let parent = tempfile::tempdir().unwrap();
    let folder = plugin_folder(
        parent.path(),
        "review-kit",
        &[("agents/reviewer.md", &agent_file("reviewer", ""))],
    );
    let data = app.data.path().to_owned();
    on_cli(move || import_plugin(&data, &folder)).await;
    run_in_yolo(&app, "go").await;
    let all = bodies(&app.llm).await;
    assert!(all.iter().all(|body| !is_subagent(body)), "nothing started");
    let result = spawn_result(&all);
    assert!(result.contains("no-such-profile"), "{result}");
    assert!(result.contains("reviewer"), "names the choices: {result}");
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_disabled_plugins_profiles_are_not_available() {
    let app = app(
        None,
        spawning(
            json!([{"write_paths": [], "name": "Review", "brief": "x", "profile": "reviewer"}]),
            |_| reply("never"),
        ),
    )
    .await;
    let parent = tempfile::tempdir().unwrap();
    let folder = plugin_folder(
        parent.path(),
        "review-kit",
        &[("agents/reviewer.md", &agent_file("reviewer", ""))],
    );
    let data = app.data.path().to_owned();
    on_cli(move || {
        import_plugin(&data, &folder);
        let (ok, _, error) = themis(&data, &["plugin", "disable", "review-kit"]);
        assert!(ok, "{error}");
    })
    .await;
    run_in_yolo(&app, "go").await;
    let all = bodies(&app.llm).await;
    assert!(all.iter().all(|body| !is_subagent(body)), "nothing started");
    let spawn = spawn_tool(all.first().unwrap());
    assert!(
        !spawn["description"].as_str().unwrap().contains("reviewer"),
        "not offered: {spawn}"
    );
    assert!(
        spawn["parameters"]["properties"]["subagents"]["items"]["properties"]["profile"].is_null(),
        "{spawn}"
    );
    let result = spawn_result(&all);
    assert!(result.contains("reviewer"), "{result}");
    assert!(result.to_lowercase().contains("profile"), "{result}");
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profiles_model_maps_to_the_models_list_or_the_default_applies() {
    let app = app(
        None,
        spawning(
            json!([
                {"write_paths": [], "name": "Listed", "brief": "a", "profile": "listed"},
                {"write_paths": [], "name": "Unlisted", "brief": "b", "profile": "unlisted"},
                {"write_paths": [], "name": "Inherit", "brief": "c", "profile": "inherits"},
                {"write_paths": [], "name": "Asked", "brief": "d", "profile": "listed", "model": "test-model"}
            ]),
            |_| reply("ok"),
        ),
    )
    .await;
    let parent = tempfile::tempdir().unwrap();
    let folder = plugin_folder(
        parent.path(),
        "model-kit",
        &[
            ("agents/listed.md", &agent_file("listed", "model: haiku\n")),
            (
                "agents/unlisted.md",
                &agent_file("unlisted", "model: opus\n"),
            ),
            (
                "agents/inherits.md",
                &agent_file("inherits", "model: inherit\n"),
            ),
        ],
    );
    let data = app.data.path().to_owned();
    on_cli(move || {
        allow_models(&data, &["claude-haiku-4"]);
        import_plugin(&data, &folder);
    })
    .await;
    run_in_yolo(&app, "go").await;
    let listed = call(
        app.data.path(),
        "list_subagents",
        json!({"threadId": app.thread}),
    );
    let model_of = |name: &str| {
        listed
            .as_array()
            .unwrap()
            .iter()
            .find(|agent| agent["name"] == name)
            .unwrap_or_else(|| panic!("{name}: {listed}"))["model"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(model_of("Listed"), "claude-haiku-4");
    assert_eq!(model_of("Unlisted"), "test-model");
    assert_eq!(model_of("Inherit"), "test-model");
    // What the main agent asks for wins over what the profile says.
    assert_eq!(model_of("Asked"), "test-model");
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_subagent_definitions_fail_the_import() {
    let data = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let serving = tokio::spawn(Server::bind(state, data.path()).await.unwrap().run());
    let parent = tempfile::tempdir().unwrap();
    let cases = [
        ("unclosed", "---\nname: x\ndescription: y\nbody"),
        ("no-frontmatter", "Just some text."),
        ("no-name", "---\ndescription: y\n---\nbody"),
        ("no-description", "---\nname: x\n---\nbody"),
        (
            "unsafe-name",
            "---\nname: ../../evil\ndescription: y\n---\nbody",
        ),
        ("empty-body", "---\nname: x\ndescription: y\n---\n"),
        (
            "unclosed-list",
            "---\nname: x\ndescription: y\ntools: [Read, Bash\n---\nbody",
        ),
        (
            "two-lists",
            "---\nname: x\ndescription: y\ntools: Read\ntools: Bash\n---\nbody",
        ),
    ];
    for (case, content) in cases {
        let folder = plugin_folder(
            parent.path(),
            case,
            &[
                ("agents/one.md", content),
                (
                    "skills/s/SKILL.md",
                    "---\nname: s\ndescription: d\n---\nUse.",
                ),
            ],
        );
        let path = data.path().to_owned();
        let (ok, plugin, error) = on_cli(move || {
            themis(
                &path,
                &[
                    "plugin",
                    "import",
                    folder.to_str().unwrap(),
                    "--allow-partial",
                ],
            )
        })
        .await;
        assert!(!ok, "{case} should fail: {plugin}");
        assert!(
            error.contains("agents/one.md"),
            "{case} names the file: {error}"
        );
    }
    let path = data.path().to_owned();
    let (_, listed, _) = on_cli(move || themis(&path, &["plugin", "list"])).await;
    assert_eq!(listed, json!([]), "nothing was saved");
    // Two files with one name are a bad definition too.
    let folder = plugin_folder(
        parent.path(),
        "duplicates",
        &[
            ("agents/one.md", &agent_file("same", "")),
            ("agents/two.md", &agent_file("same", "")),
        ],
    );
    let path = data.path().to_owned();
    let (ok, plugin, _) =
        on_cli(move || themis(&path, &["plugin", "import", folder.to_str().unwrap()])).await;
    assert!(!ok, "{plugin}");
    serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn subagent_paths_that_leave_the_plugin_are_not_imported() {
    let data = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let serving = tokio::spawn(Server::bind(state, data.path()).await.unwrap().run());
    let parent = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(parent.path().join("outside")).unwrap();
    std::fs::write(
        parent.path().join("outside/evil.md"),
        agent_file("evil", ""),
    )
    .unwrap();
    let folder = plugin_folder(
        parent.path(),
        "escape-kit",
        &[("agents/good.md", &agent_file("good", ""))],
    );
    std::fs::write(
        folder.join(".claude-plugin/plugin.json"),
        json!({"name": "escape-kit", "agents": ["../outside/evil.md", "./agents/../../outside/"]})
            .to_string(),
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        parent.path().join("outside/evil.md"),
        folder.join("agents/link.md"),
    )
    .unwrap();
    let path = data.path().to_owned();
    let (ok, plugin, error) = on_cli(move || {
        themis(
            &path,
            &[
                "plugin",
                "import",
                folder.to_str().unwrap(),
                "--allow-partial",
            ],
        )
    })
    .await;
    assert!(ok, "{error}");
    assert_eq!(profile_names(&plugin), ["good"], "{plugin}");
    let unsupported = plugin["spec"]["unsupported"].to_string();
    assert!(unsupported.contains("outside the plugin"), "{unsupported}");
    #[cfg(unix)]
    assert!(unsupported.contains("Link not followed"), "{unsupported}");
    serving.abort();
}

/// A sorted tool list as a subagent gets it: on Windows, which runs commands without a shell,
/// reading also brings the dedicated listing and search tools.
fn with_reads(names: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
    if cfg!(windows) && names.iter().any(|name| name == "read_file") {
        names.extend(["list_dir".to_owned(), "search_file".to_owned()]);
    }
    names.sort();
    names
}
