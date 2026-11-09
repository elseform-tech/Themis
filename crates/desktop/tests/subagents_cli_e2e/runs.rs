//! Subagent runs: guidance, skills, notes, stopping, write paths, settings, approvals and models.

use super::*;

/// A subagent does the work a skill describes, so it needs the list as much as the main agent.
#[tokio::test(flavor = "multi_thread")]
async fn a_subagent_can_see_the_skills_the_main_agent_can() {
    let app = app(None, |body| {
        if is_subagent(body) {
            return reply("Done.");
        }
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [{"write_paths": [], "name": "Builder", "brief": "Build it."}]}),
            )]),
            _ if mentions(body, "Done.") => reply("All done."),
            _ => reply("Waiting."),
        }
    })
    .await;
    let skill = app.project.path().join(".agents/skills/test-first-probe");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: test-first-probe\ndescription: Write the failing test before the code\n---\nTests first.\n",
    )
    .unwrap();
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || run(&data, &thread, "build it"))
        .await
        .unwrap();
    let requests = bodies(&app.llm).await;
    let has_skill = |body: &&Value| {
        messages(body)
            .iter()
            .any(|m| text(m).contains("test-first-probe"))
    };
    assert!(
        requests
            .iter()
            .filter(|b| !is_subagent(b))
            .any(|b| has_skill(&b)),
        "the main agent's list has the skill"
    );
    assert!(
        requests
            .iter()
            .filter(|b| is_subagent(b))
            .any(|b| has_skill(&b)),
        "a subagent's list has it too"
    );
}

/// The project's own guidance reaches an agent and a subagent without anyone configuring it, with a
/// map of the other docs, since AGENTS.md is often thin.
#[tokio::test(flavor = "multi_thread")]
async fn agents_and_subagents_read_the_projects_guidance_and_get_a_map_of_its_docs() {
    let app = app(None, |body| {
        if is_subagent(body) {
            return reply("Done.");
        }
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [{"write_paths": [], "name": "Builder", "brief": "Build it."}]}),
            )]),
            _ if mentions(body, "Done.") => reply("All done."),
            _ => reply("Waiting."),
        }
    })
    .await;
    let root = app.project.path();
    std::fs::write(root.join("AGENTS.md"), "HOUSE RULE 77: tests first.").unwrap();
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("docs/guide.md"), "UNLOADED GUIDE BODY").unwrap();
    std::fs::write(root.join("README.md"), "UNLOADED README BODY").unwrap();
    std::fs::create_dir_all(root.join("part/deep")).unwrap();
    std::fs::write(root.join("part/AGENTS.md"), "UNLOADED PART RULES").unwrap();
    std::fs::create_dir_all(root.join("node_modules/x")).unwrap();
    std::fs::write(root.join("node_modules/x/README.md"), "dependency").unwrap();
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || run(&data, &thread, "build it"))
        .await
        .unwrap();
    let requests = bodies(&app.llm).await;
    for body in [
        requests.iter().find(|b| !is_subagent(b)).unwrap(),
        requests.iter().find(|b| is_subagent(b)).unwrap(),
    ] {
        assert!(mentions(body, "HOUSE RULE 77"), "AGENTS.md is read");
        assert!(mentions(body, "docs/guide.md"), "the docs are on the map");
        assert!(mentions(body, "README.md") && mentions(body, "part/AGENTS.md"));
        assert!(
            !mentions(body, "UNLOADED"),
            "the map names them and doesn't load them"
        );
        assert!(!mentions(body, "node_modules"), "not a dependency's docs");
    }
}

/// The main agent is told integrations are managed with `themis` in its shell, and a subagent
/// with an open shell that it may read them that way; neither gets the other's line.
#[tokio::test(flavor = "multi_thread")]
async fn agents_are_told_where_integrations_are_managed() {
    let app = app(None, |body| {
        if is_subagent(body) {
            return reply("Done.");
        }
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [{"write_paths": [], "name": "Reader", "brief": "Look."}]}),
            )]),
            _ if mentions(body, "Done.") => reply("All done."),
            _ => reply("Waiting."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || run(&data, &thread, "look"))
        .await
        .unwrap();
    let requests = bodies(&app.llm).await;
    let main = requests.iter().find(|b| !is_subagent(b)).unwrap();
    let child = requests.iter().find(|b| is_subagent(b)).unwrap();
    let main_system = text(&messages(main)[0]);
    let child_system = text(&messages(child)[0]);
    assert!(
        main_system.contains(themis_core::prompts::DESKTOP_RUN_INTEGRATIONS),
        "{main_system}"
    );
    assert!(!main_system.contains(themis_core::prompts::SUBAGENT_INTEGRATIONS_NOTE));
    assert_eq!(
        child_system.contains(themis_core::prompts::SUBAGENT_INTEGRATIONS_NOTE),
        tool_names(child).iter().any(|name| name == "shell"),
        "{child_system}"
    );
    if !cfg!(windows) {
        assert!(child_system.contains(themis_core::prompts::SUBAGENT_INTEGRATIONS_NOTE));
    }
    assert!(!child_system.contains(themis_core::prompts::DESKTOP_RUN_INTEGRATIONS));
}

/// CLAUDE.md is read when there is no AGENTS.md, and the setting turns all of this off.
#[tokio::test(flavor = "multi_thread")]
async fn claude_md_stands_in_and_the_setting_turns_guidance_off() {
    let app_on = app(None, |_| reply("Done.")).await;
    std::fs::write(app_on.project.path().join("CLAUDE.md"), "CLAUDE RULE 88").unwrap();
    let (data, thread) = (app_on.data.path().to_owned(), app_on.thread.clone());
    tokio::task::spawn_blocking(move || run(&data, &thread, "hello"))
        .await
        .unwrap();
    assert!(mentions(&bodies(&app_on.llm).await[0], "CLAUDE RULE 88"));

    let app_off = app(Some(r#"{"project_guidance":false}"#), |_| reply("Done.")).await;
    std::fs::write(app_off.project.path().join("AGENTS.md"), "OFF RULE 99").unwrap();
    let (data, thread) = (app_off.data.path().to_owned(), app_off.thread.clone());
    tokio::task::spawn_blocking(move || run(&data, &thread, "hello"))
        .await
        .unwrap();
    assert!(!mentions(&bodies(&app_off.llm).await[0], "OFF RULE 99"));
}

/// What superpowers and similar plugins tell agents to call. Loading a skill must be the easy way,
/// for an agent and a subagent, by the plugin's name for it or the skill's own.
#[tokio::test(flavor = "multi_thread")]
async fn the_skill_tool_loads_a_skill_for_an_agent_and_a_subagent() {
    let results: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = Arc::clone(&results);
    let app = app(None, move |body| {
        let done = tool_results(body);
        if is_subagent(body) {
            assert!(
                mentions(body, "load it with the Skill tool"),
                "told it applies"
            );
            return match done {
                0 => tools(&[(
                    "b1",
                    "Skill",
                    json!({"skill": "test-first-probe", "status": "Loading"}),
                )]),
                _ => {
                    seen.lock().unwrap().extend(
                        messages(body)
                            .iter()
                            .filter(|m| m["role"] == "tool")
                            .map(|m| text(m).to_owned()),
                    );
                    reply("Done.")
                }
            };
        }
        match done {
            0 => tools(&[(
                "a1",
                "Skill",
                json!({"skill": "superpowers:test-first-probe"}),
            )]),
            1 => tools(&[("a2", "Skill", json!({"skill": "no-such-skill"}))]),
            2 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [{"write_paths": [], "name": "Builder", "brief": "Build it."}]}),
            )]),
            _ if mentions(body, "Done.") => {
                seen.lock().unwrap().extend(
                    messages(body)
                        .iter()
                        .filter(|m| m["role"] == "tool")
                        .map(|m| text(m).to_owned()),
                );
                reply("All done.")
            }
            _ => reply("Waiting."),
        }
    })
    .await;
    let skill = app.project.path().join(".agents/skills/test-first-probe");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: test-first-probe\ndescription: Write the failing test before the code\n---\nALWAYS WRITE THE TEST FIRST.\n",
    )
    .unwrap();
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || run(&data, &thread, "build it"))
        .await
        .unwrap();
    let requests = bodies(&app.llm).await;
    let main = requests.iter().find(|b| !is_subagent(b)).unwrap();
    assert!(
        tool_names(main).iter().any(|n| n == "Skill"),
        "{:?}",
        tool_names(main)
    );
    let child = requests.iter().find(|b| is_subagent(b)).unwrap();
    assert!(
        tool_names(child).iter().any(|n| n == "Skill"),
        "{:?}",
        tool_names(child)
    );
    let results = results.lock().unwrap().join("\n");
    assert!(results.contains("ALWAYS WRITE THE TEST FIRST"), "{results}");
    assert!(
        results.contains("no-such-skill") && results.contains("test-first-probe"),
        "says what exists: {results}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_turn_ends_after_both_subagents_reported_back() {
    let app = app(None, |body| {
        let last = messages(body)
            .iter()
            .rfind(|m| m["role"] == "user")
            .map(text)
            .unwrap_or_default()
            .to_owned();
        if is_subagent(body) {
            let results = tool_results(body);
            return if last.contains("Read notes.txt") {
                match results {
                    0 => tools(&[(
                        "a1",
                        "read_file",
                        json!({"file_path": "notes.txt", "status": "Reading the notes"}),
                    )]),
                    _ => reply("A: the notes say hello."),
                }
            } else {
                match results {
                    0 => tools(&[(
                        "b1",
                        "list_dir",
                        json!({"path": ".", "status": "Listing the folder"}),
                    )]),
                    _ => reply("B: one file, notes.txt."),
                }
            };
        }
        let both = mentions(body, "A: the notes say hello.") && mentions(body, "B: one file");
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [
                    {"write_paths": [], "name": "Notes", "brief": "Read notes.txt and say what it says."},
                    {"write_paths": [], "name": "Files", "brief": "List the project folder."}
                ]}),
            )]),
            _ if both => reply("Merged: hello, and one file."),
            _ => reply("Waiting for the subagents."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || run(&data, &thread, "review the project")
    })
    .await
    .unwrap();

    let all = bodies(&app.llm).await;
    let main: Vec<&Value> = all.iter().filter(|body| !is_subagent(body)).collect();
    let children: Vec<&Value> = all.iter().filter(|body| is_subagent(body)).collect();
    // The main agent saw both results, labelled as a subagent's, before it finished.
    let last = main.last().unwrap();
    assert!(mentions(last, NOTE), "results arrive labelled");
    assert!(mentions(last, REVIEW), "and with the request to check them");
    assert!(mentions(last, "A: the notes say hello.") && mentions(last, "B: one file"));
    // Subagents get only read-only and web tools with a status field, and no subagent tools.
    for child in &children {
        let names = tool_names(child);
        assert!(
            names.iter().all(|name| name == "Skill"
                || [
                    "read_file",
                    "list_dir",
                    "search_file",
                    "shell",
                    "fetch_url",
                    "web_search"
                ]
                .contains(&name.as_str())),
            "{names:?}"
        );
        let read = child["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["function"]["name"] == "read_file")
            .unwrap();
        assert_eq!(
            read["function"]["parameters"]["properties"]["status"]["maxLength"],
            80
        );
    }
    assert!(tool_names(main[0]).contains(&"spawn_subagents".to_owned()));
    // A read_file call with a status line still read the file.
    assert!(children
        .iter()
        .any(|child| mentions(child, "hello from the notes")));

    tokio::task::spawn_blocking(move || {
        let reply = call(&data, "get_thread", json!({"threadId": thread}));
        let events = history_kinds(&data, &thread);
        let started = events
            .iter()
            .filter(|e| e["kind"] == "subagent_started")
            .count();
        let ended: Vec<_> = events
            .iter()
            .filter(|e| e["kind"] == "subagent_ended")
            .collect();
        assert_eq!((started, ended.len()), (2, 2), "{events:?} {reply}");
        assert!(ended.iter().all(|e| e["status"] == "finished"), "{ended:?}");
        assert!(ended.iter().all(|e| e["tool_calls"] == 1));
        let listed = call(&data, "list_subagents", json!({"threadId": thread}));
        let listed = listed.as_array().unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed
            .iter()
            .any(|agent| agent["text"] == "Reading the notes"));
        // Each subagent's own events are saved beside the chat.
        let saved = std::fs::read_dir(data.join("subagents").join(&thread))
            .map(|dir| dir.count())
            .unwrap_or(0);
        assert_eq!(saved, 2);
        // The read-only detail reads a subagent's own steps.
        let id = listed[0]["id"].as_str().unwrap();
        let steps = call(
            &data,
            "get_subagent_transcript",
            json!({"threadId": thread, "subagentId": id}),
        );
        let kinds: Vec<&str> = steps
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|event| event["kind"].as_str())
            .collect();
        assert!(
            kinds.contains(&"tool_started") && kinds.contains(&"finished"),
            "{kinds:?}"
        );
        // Deleting the chat removes them.
        call(&data, "discard_thread", json!({"threadId": thread}));
        assert!(!data.join("subagents").join(&thread).exists());
    })
    .await
    .unwrap();
    app.serving.abort();
}

/// Starting a subagent doesn't hold the main agent: while a slow subagent works, the main agent's
/// next step runs a tool and gets its result, and a reply without tools doesn't end the turn
/// until the subagent's report has reached it.
#[tokio::test(flavor = "multi_thread")]
async fn the_main_agent_keeps_working_while_a_slow_subagent_runs() {
    const SLOW: Duration = Duration::from_secs(4);
    const REPORT: &str = "Slow: everything looked at.";
    // When each request arrived: (main agent?, its tool results, the time).
    let arrivals: Arc<Mutex<Vec<(bool, usize, Instant)>>> = Arc::default();
    let log = Arc::clone(&arrivals);
    let app = app(None, move |body| {
        let subagent = is_subagent(body);
        log.lock()
            .unwrap()
            .push((!subagent, tool_results(body), Instant::now()));
        if subagent {
            return reply(REPORT).set_delay(SLOW);
        }
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [{"write_paths": [], "name": "Slow", "brief": "Look at everything."}]}),
            )]),
            1 => tools(&[("r1", "read_file", json!({"file_path": "notes.txt"}))]),
            _ if mentions(body, REPORT) => reply("Merged: Slow's report and the notes."),
            _ => reply("The notes say hello; Slow is still looking."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || run(&data, &thread, "look around")
    })
    .await
    .unwrap();

    let all = bodies(&app.llm).await;
    let main: Vec<&Value> = all.iter().filter(|body| !is_subagent(body)).collect();
    // The main agent's own read came back while the subagent was still working: that request
    // carries the read's result and no report yet.
    let read_back = main
        .iter()
        .find(|body| mentions(body, "hello from the notes"))
        .expect("the main agent's read never came back");
    assert!(!mentions(read_back, NOTE), "{read_back}");
    // And on time: well before the subagent's answer was due.
    let arrivals = arrivals.lock().unwrap().clone();
    let child_asked = arrivals
        .iter()
        .find(|(is_main, _, _)| !is_main)
        .map(|(_, _, at)| *at)
        .expect("the subagent never asked its model");
    let read_at = arrivals
        .iter()
        .find(|(is_main, results, _)| *is_main && *results == 2)
        .map(|(_, _, at)| *at)
        .expect("no main step after the read");
    assert!(
        read_at < child_asked + SLOW,
        "the main agent waited for the subagent: {:?} after it asked",
        read_at.saturating_duration_since(child_asked)
    );
    // Its reply without tools while the subagent ran did not end the turn: the turn went on
    // until the report arrived, labelled as the subagent's.
    let last = main.last().unwrap();
    assert!(mentions(last, "Slow is still looking"), "{last}");
    assert!(mentions(last, NOTE) && mentions(last, REPORT), "{last}");
    tokio::task::spawn_blocking(move || {
        let ended: Vec<_> = history_kinds(&data, &thread)
            .into_iter()
            .filter(|e| e["kind"] == "subagent_ended")
            .collect();
        assert_eq!(ended.len(), 1, "{ended:?}");
        assert_eq!(ended[0]["status"], "finished");
    })
    .await
    .unwrap();
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_subagent_ends_and_a_waited_result_is_not_repeated() {
    let app = app(None, move |body| {
        if is_subagent(body) {
            // Still busy when the main agent stops it.
            return tools(&[("c1", "list_dir", json!({"path": "."}))])
                .set_delay(Duration::from_secs(3));
        }
        let id = spawned_id(body).unwrap_or_default();
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [{"write_paths": [], "name": "Slow", "brief": "Look at everything."}]}),
            )]),
            // In one reply: notes join between replies, so a subagent that ends quickly (as it
            // did on Windows) can't reach the main agent as a note before the wait takes it.
            1 => tools(&[
                ("t1", "stop_subagent", json!({"id": id})),
                (
                    "w1",
                    "wait_subagents",
                    json!({"ids": [id], "timeout_seconds": 30}),
                ),
            ]),
            _ => reply("It was stopped."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || run(&data, &thread, "look around")
    })
    .await
    .unwrap();
    let all = bodies(&app.llm).await;
    let main: Vec<&Value> = all.iter().filter(|body| !is_subagent(body)).collect();
    let last = main.last().unwrap();
    // wait_subagents returned the result, so it never came again as a note.
    assert!(mentions(last, r#""status":"stopped""#), "{last}");
    // A result returned by wait_subagents carries the same request to check it.
    assert!(mentions(last, REVIEW), "{last}");
    assert!(!main.iter().any(|body| mentions(body, NOTE)));
    tokio::task::spawn_blocking(move || {
        let events = history_kinds(&data, &thread);
        let ended: Vec<_> = events
            .iter()
            .filter(|e| e["kind"] == "subagent_ended")
            .collect();
        assert_eq!(ended.len(), 1, "{events:?}");
        assert_eq!(ended[0]["status"], "stopped");
    })
    .await
    .unwrap();
    app.serving.abort();
}

/// A call too big for a subagent's context, such as a whole file written through a heredoc, is
/// refused and saved as for the main agent, rather than ending the subagent.
#[tokio::test(flavor = "multi_thread")]
async fn a_very_long_tool_call_doesnt_end_a_subagent() {
    let app = app(Some(r#"{"context_token_budget":20000}"#), |body| {
        if is_subagent(body) {
            return match tool_results(body) {
                0 => tools(&[(
                    "c1",
                    "shell",
                    json!({"command": format!("cat << 'EOF' > notes/a.txt\n{}\nEOF", "line ".repeat(30_000))}),
                )]),
                _ => reply("Reviewed it all."),
            };
        }
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [{"name": "Big", "brief": "Run it.", "write_paths": ["notes/"]}]}),
            )]),
            _ if mentions(body, "Reviewed it all.") => reply("All done."),
            _ if mentions(body, "exceeds the delivery allowance") => reply("It failed."),
            _ => tools(&[("w1", "wait_subagents", json!({"timeout_seconds": 30}))]),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || run(&data, &thread, "build it"))
        .await
        .unwrap();
    let requests = bodies(&app.llm).await;
    let last = requests.iter().rfind(|b| !is_subagent(b)).unwrap();
    assert!(mentions(last, "Reviewed it all."), "{last}");
    assert!(!mentions(last, "exceeds the delivery allowance"), "{last}");
}

/// A model that leaves out `write_paths` learns at once what that means, rather than from a
/// subagent coming back unable to build anything.
#[tokio::test(flavor = "multi_thread")]
async fn starting_a_subagent_says_when_it_can_only_read() {
    let app = app(None, |body| {
        if is_subagent(body) {
            return reply("Done.");
        }
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [
                    {"write_paths": [], "name": "Looker", "brief": "Look around."},
                    {"name": "Builder", "brief": "Build it.", "write_paths": ["notes/"]}
                ]}),
            )]),
            1 => tools(&[(
                "s2",
                "spawn_subagents",
                json!({"subagents": [
                    {"name": "Maker", "brief": "Make it.", "write_paths": ["more/"]}
                ]}),
            )]),
            _ if mentions(body, "Done.") => reply("All done."),
            _ => reply("Waiting."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || run(&data, &thread, "build it"))
        .await
        .unwrap();
    let requests = bodies(&app.llm).await;
    let last = requests.iter().rfind(|b| !is_subagent(b)).unwrap();
    let results: Vec<&str> = messages(last)
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(text)
        .filter(|text| text.contains("started"))
        .collect();
    assert!(
        results[0].contains("Looker can only read"),
        "{}",
        results[0]
    );
    assert!(results[0].contains("write_paths") && !results[0].contains("Builder can only"));
    assert!(!results[1].contains("can only read"), "{}", results[1]);
}

/// Two read-only subagents and one that owns `notes/`: it writes there and nowhere else, in
/// YOLO too, and the others stay read-only. Overlaps and escapes are refused before any start.
#[tokio::test(flavor = "multi_thread")]
async fn write_paths_hold_a_subagent_to_its_own_folder() {
    let app = app(None, |body| {
        if is_subagent(body) {
            if !mentions(body, "Write the summary") {
                return reply("Read-only part done.");
            }
            return match tool_results(body) {
                // Slow enough that the main agent's next spawn finds it running.
                0 => tools(&[(
                    "w1",
                    "write_file",
                    json!({"file_path": "notes/summary.md", "content": "inside", "status": "Writing"}),
                )])
                .set_delay(std::time::Duration::from_millis(1500)),
                1 => tools(&[(
                    "w2",
                    "write_file",
                    json!({"file_path": "outside.md", "content": "outside", "status": "Trying"}),
                )]),
                2 => tools(&[(
                    "w3",
                    "shell",
                    json!({"command": "sh", "args": ["-c", "echo shell > notes/shell.md; echo shell > shell-outside.md"], "status": "Shell"}),
                )]),
                _ => reply("Writer done."),
            };
        }
        match tool_results(body) {
            0 => tools(&[(
                "bad1",
                "spawn_subagents",
                json!({"subagents": [{"brief": "Escape", "write_paths": ["../up/"]}]}),
            )]),
            1 => tools(&[(
                "bad2",
                "spawn_subagents",
                json!({"subagents": [{"brief": "Settings", "write_paths": [".themis/"]}]}),
            )]),
            2 => tools(&[(
                "bad3",
                "spawn_subagents",
                json!({"subagents": [
                    {"brief": "One", "write_paths": ["notes/"]},
                    {"brief": "Two", "write_paths": ["notes/deep/file.md"]}
                ]}),
            )]),
            3 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [
                    {"write_paths": [], "name": "Reader A", "brief": "Read part A"},
                    {"write_paths": [], "name": "Reader B", "brief": "Read part B"},
                    {"name": "Writer", "brief": "Write the summary", "write_paths": ["notes/"]}
                ]}),
            )]),
            4 => tools(&[(
                "s2",
                "spawn_subagents",
                json!({"subagents": [{"brief": "Late", "write_paths": ["notes/late.md"]}]}),
            )]),
            5 => tools(&[("w", "wait_subagents", json!({}))]),
            _ => reply("All done."),
        }
    })
    .await;
    run(app.data.path(), &app.thread, "Do the parts");
    let root = app.project.path();
    assert_eq!(
        std::fs::read_to_string(root.join("notes/summary.md")).unwrap(),
        "inside"
    );
    assert!(!root.join("outside.md").exists(), "the file tool stayed in");
    assert!(!root.join("up").exists());

    let all = bodies(&app.llm).await;
    let main_results = |needle: &str| {
        all.iter().any(|body| {
            !is_subagent(body)
                && messages(body)
                    .iter()
                    .any(|m| m["role"] == "tool" && text(m).contains(needle))
        })
    };
    assert!(main_results("must stay inside the project"));
    assert!(main_results("never writable by a subagent"));
    assert!(main_results("overlaps"), "within one call");
    assert!(main_results("which Writer owns"), "with a running one");

    let writer_tools = all
        .iter()
        .find(|body| is_subagent(body) && mentions(body, "Write the summary"))
        .map(tool_names)
        .unwrap();
    assert!(writer_tools.iter().any(|name| name == "write_file"));
    assert!(writer_tools.iter().any(|name| name == "apply_patch"));
    assert!(!writer_tools.iter().any(|name| name == "delete_file"));
    assert!(all.iter().any(|body| is_subagent(body)
        && mentions(
            body,
            "may change files only under these project paths: notes/"
        )));
    let reader = all
        .iter()
        .find(|body| is_subagent(body) && mentions(body, "Read part A"))
        .unwrap();
    assert!(!tool_names(reader).iter().any(|name| name == "write_file"));
    assert!(mentions(reader, "You are read-only"));

    let writer_outcome = |needle: &str| {
        all.iter().any(|body| {
            is_subagent(body)
                && messages(body)
                    .iter()
                    .any(|m| m["role"] == "tool" && text(m).contains(needle))
        })
    };
    assert!(
        writer_outcome("outside the folders"),
        "the refusal reaches it"
    );
    // The shell wall needs an enforcing sandbox: macOS always has one.
    if cfg!(target_os = "macos") {
        assert!(
            !root.join("shell-outside.md").exists(),
            "the shell stayed in"
        );
        assert!(
            root.join("notes/shell.md").exists(),
            "its own folder is writable"
        );
    }

    let events = history_kinds(app.data.path(), &app.thread);
    let started = events
        .iter()
        .find(|event| event["kind"] == "subagent_started" && event["name"] == "Writer")
        .expect("the writer started");
    assert_eq!(started["write_paths"], json!(["notes/"]));
    app.serving.abort();
}

/// The main agent sends a finished subagent back to correct itself: same subagent, its earlier
/// exchange as history, and a second report. A later turn can do the same.
#[tokio::test(flavor = "multi_thread")]
async fn the_main_agent_can_send_a_finished_subagent_back() {
    let id = Arc::new(Mutex::new(String::new()));
    let known = Arc::clone(&id);
    let app = app(None, move |body| {
        if is_subagent(body) {
            return if mentions(body, "That is wrong") {
                reply("Corrected answer: 42.")
            } else {
                reply("First answer: 41.")
            };
        }
        if tool_names(body).is_empty() {
            return reply("Hmm, let me look at that.");
        }
        if !mentions(body, "Ask it again") {
            return match tool_results(body) {
                0 => tools(&[(
                    "s1",
                    "spawn_subagents",
                    json!({"say": "Starting Adder to do the sum.", "subagents": [{"write_paths": [], "name": "Adder", "brief": "Add the numbers"}]}),
                )]),
                1 => tools(&[("w1", "wait_subagents", json!({}))]),
                2 => tools(&[("m1", "message_subagent", {
                    let found = spawned_id(body).unwrap();
                    *known.lock().unwrap() = found.clone();
                    json!({"id": found, "message": "That is wrong, check the sum"})
                })]),
                3 => tools(&[("w2", "wait_subagents", json!({}))]),
                _ => reply("Now it says 42."),
            };
        }
        // A later turn: its tool results start again, and the id comes from the first turn.
        match tool_results(body) {
            0 => tools(&[(
                "m2",
                "message_subagent",
                json!({"id": known.lock().unwrap().clone(), "message": "One more: double it"}),
            )]),
            1 => tools(&[("w3", "wait_subagents", json!({}))]),
            _ => reply("It doubled it."),
        }
    })
    .await;
    run(app.data.path(), &app.thread, "Do the sum");
    let all = bodies(&app.llm).await;
    let second = all
        .iter()
        .find(|body| is_subagent(body) && mentions(body, "That is wrong"))
        .expect("it was sent back");
    assert!(
        mentions(second, "Add the numbers"),
        "its brief is still in its history"
    );
    assert!(mentions(second, "First answer: 41."), "so is its answer");
    assert!(all
        .iter()
        .any(|body| !is_subagent(body) && mentions(body, "Corrected answer: 42.")));
    // After each report the main agent had a step with no tools, to say what it made of it.
    assert_eq!(
        all.iter()
            .filter(|body| !is_subagent(body) && tool_names(body).is_empty())
            .count(),
        2
    );
    assert!(all
        .iter()
        .all(|body| !is_subagent(body) || !mentions(body, "A subagent just reported back")));
    let events = history_kinds(app.data.path(), &app.thread);
    let kinds: Vec<&str> = events
        .iter()
        .filter_map(|event| event["kind"].as_str())
        .filter(|kind| *kind != "subagent_status")
        .collect();
    assert_eq!(
        kinds,
        [
            "subagent_started",
            "subagent_ended",
            "subagent_message",
            "subagent_ended"
        ]
    );
    let message = events
        .iter()
        .find(|event| event["kind"] == "subagent_message")
        .unwrap();
    assert_eq!(message["resumed"], json!(true));
    // The line on a subagent tool shows in the chat before the tool starts.
    let history = call(
        app.data.path(),
        "get_thread_history",
        json!({"threadId": app.thread}),
    );
    let kinds: Vec<Value> = history
        .as_array()
        .or_else(|| history["items"].as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|item| item["envelope"]["event"].clone())
        .collect();
    let said = kinds
        .iter()
        .position(|event| {
            event["kind"] == "assistant_text"
                && event["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("Starting Adder"))
        })
        .expect("the line was shown");
    let started = kinds
        .iter()
        .position(|event| event["kind"] == "tool_started" && event["tool"] == "spawn_subagents")
        .unwrap();
    assert!(said < started);

    run(app.data.path(), &app.thread, "Ask it again");
    let all = bodies(&app.llm).await;
    let third = all
        .iter()
        .find(|body| is_subagent(body) && mentions(body, "One more: double it"))
        .expect("a later turn reached it too");
    assert!(mentions(third, "Corrected answer: 42."));
    app.serving.abort();
}

/// Ordinary tool calls never lose their tools for a step.
#[tokio::test(flavor = "multi_thread")]
async fn plain_tool_calls_are_not_nudged() {
    let app = app(None, |body| match tool_results(body) {
        0 => tools(&[("r1", "read_file", json!({"file_path": "notes.txt"}))]),
        1 => tools(&[("r2", "list_dir", json!({"path": "."}))]),
        _ => reply("Done."),
    })
    .await;
    run(app.data.path(), &app.thread, "Look around");
    assert!(!bodies(&app.llm)
        .await
        .iter()
        .any(|body| tool_names(body).is_empty()));
    app.serving.abort();
}

/// `subagents.allow_writes` off keeps a subagent read-only and says so; a project can do this.
#[tokio::test(flavor = "multi_thread")]
async fn a_project_can_turn_subagent_writes_off() {
    let app = app(Some(r#"{"subagents": {"allow_writes": false}}"#), |body| {
        if is_subagent(body) {
            return reply("Done.");
        }
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [{"brief": "Write it", "write_paths": ["notes/"]}]}),
            )]),
            1 => tools(&[("w", "wait_subagents", json!({}))]),
            _ => reply("All done."),
        }
    })
    .await;
    run(app.data.path(), &app.thread, "Do it");
    let all = bodies(&app.llm).await;
    let child = all.iter().find(|body| is_subagent(body)).unwrap();
    assert!(!tool_names(child).iter().any(|name| name == "write_file"));
    assert!(mentions(child, "You are read-only"));
    assert!(all.iter().any(|body| {
        !is_subagent(body)
            && messages(body)
                .iter()
                .any(|m| m["role"] == "tool" && text(m).contains("write_paths were ignored"))
    }));
    assert!(!app.project.path().join("notes").exists());
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_can_turn_subagents_off() {
    let app = app(Some(r#"{"subagents": {"enabled": false}}"#), |_| {
        reply("Done.")
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || run(&data, &thread, "hello"))
        .await
        .unwrap();
    let all = bodies(&app.llm).await;
    assert!(!all.is_empty());
    assert!(all.iter().all(|body| !tool_names(body)
        .iter()
        .any(|name| name.contains("subagent"))));
    drop(app.project);
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_person_can_stop_one_subagent_and_the_agent_still_hears_from_it() {
    let app = app(None, move |body| {
        if is_subagent(body) {
            return tools(&[("c1", "list_dir", json!({"path": "."}))])
                .set_delay(Duration::from_secs(4));
        }
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [{"write_paths": [], "name": "Slow", "brief": "Look at everything."}]}),
            )]),
            _ if mentions(body, NOTE) => reply("Slow was stopped."),
            _ => reply("Waiting for Slow."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || {
            call(
                &data,
                "set_thread_approval_mode",
                json!({"threadId": thread, "mode": "yolo"}),
            );
            call(
                &data,
                "send_message",
                json!({"threadId": thread, "text": "look around"}),
            );
            let mut id = String::new();
            until("the subagent never started", || {
                let listed = call(&data, "list_subagents", json!({"threadId": thread}));
                id = listed[0]["id"].as_str().unwrap_or_default().to_owned();
                !id.is_empty()
            });
            call(
                &data,
                "stop_subagent",
                json!({"threadId": thread, "subagentId": id}),
            );
            until("the run never finished", || !running(&data, &thread));
            let listed = call(&data, "list_subagents", json!({"threadId": thread}));
            assert_eq!(listed[0]["status"], "stopped");
        }
    })
    .await
    .unwrap();
    let all = bodies(&app.llm).await;
    let main: Vec<&Value> = all.iter().filter(|body| !is_subagent(body)).collect();
    let last = main.last().unwrap();
    assert!(
        mentions(last, NOTE) && mentions(last, "status stopped"),
        "{last}"
    );
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_change_only_their_fields_and_refuse_a_stale_page() {
    let app = app(None, |_| reply("Done.")).await;
    let data = app.data.path().to_owned();
    tokio::task::spawn_blocking(move || {
        let view = call(&data, "get_subagent_settings", json!({}));
        assert_eq!(view["settings"]["max_parallel"], 3);
        let revision = view["revision"].clone();
        let saved = call(
            &data,
            "update_subagent_settings",
            json!({"changes": {"max_parallel": 2, "model": "glm-5"}, "revision": revision}),
        );
        assert_eq!(saved["settings"]["max_parallel"], 2);
        assert_eq!(saved["settings"]["model"], "glm-5");
        assert_eq!(saved["settings"]["enabled"], true);
        // The page that loaded before is stale now.
        let stale = try_call(
            &data,
            "update_subagent_settings",
            json!({"changes": {"enabled": false}, "revision": revision}),
        );
        assert!(stale.unwrap_err().contains("changed in runtime.jsonc"));
        // "Chat model" clears the field.
        let cleared = call(
            &data,
            "update_subagent_settings",
            json!({"changes": {"model": null}, "revision": saved["revision"]}),
        );
        assert!(cleared["settings"]["model"].is_null());
        // Only the fields the page shows, within their limits.
        assert!(try_call(
            &data,
            "update_subagent_settings",
            json!({"changes": {"max_turns": 5}})
        )
        .unwrap_err()
        .contains("max_turns"));
        assert!(try_call(
            &data,
            "update_subagent_settings",
            json!({"changes": {"max_parallel": 9}})
        )
        .is_err());
    })
    .await
    .unwrap();
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn starting_subagents_needs_no_prompt_but_a_deny_rule_still_holds() {
    let respond = |body: &Value| {
        if is_subagent(body) {
            return reply("Nothing to report.");
        }
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [{"write_paths": [], "name": "Quick", "brief": "Say hello."}]}),
            )]),
            _ if mentions(body, NOTE) => reply("Quick reported back."),
            _ if mentions(body, "approval denied") => reply("Subagents are not allowed here."),
            _ => reply("Waiting."),
        }
    };
    // Custom mode with no rules: no approval prompt holds the spawn up.
    let app1 = app(None, respond).await;
    let (data, thread) = (app1.data.path().to_owned(), app1.thread.clone());
    tokio::task::spawn_blocking(move || run_custom(&data, &thread, "say hello"))
        .await
        .unwrap();
    let all = bodies(&app1.llm).await;
    assert!(all
        .iter()
        .any(|body| !is_subagent(body) && mentions(body, NOTE)));
    app1.serving.abort();
    // A project rule that denies the tool refuses it.
    let app2 = app(
        Some(r#"{"approval": {"rules": [{"tool": "spawn_subagents", "action": "deny"}]}}"#),
        respond,
    )
    .await;
    let (data, thread) = (app2.data.path().to_owned(), app2.thread.clone());
    tokio::task::spawn_blocking(move || run_custom(&data, &thread, "say hello"))
        .await
        .unwrap();
    let all = bodies(&app2.llm).await;
    assert!(all.iter().all(|body| !is_subagent(body)), "no subagent ran");
    assert!(all.iter().any(|body| mentions(body, "approval denied")));
    app2.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_person_names_a_model_per_subagent_from_the_allowed_ones() {
    let app = app(None, |body| {
        if is_subagent(body) {
            return reply(&format!(
                "Ran on {}.",
                body["model"].as_str().unwrap_or("?")
            ));
        }
        match tool_results(body) {
            // A model that is not allowed is refused before anything starts.
            0 => tools(&[(
                "s0",
                "spawn_subagents",
                json!({"subagents": [{"write_paths": [], "name": "Big", "brief": "b", "model": "claude-opus"}]}),
            )]),
            1 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [
                    {"write_paths": [], "name": "Review", "brief": "Review src/.", "model": "GLM 5"},
                    {"write_paths": [], "name": "Docs", "brief": "Check docs/."}
                ]}),
            )]),
            _ if mentions(body, "Ran on glm-5.") && mentions(body, "Ran on test-model.") => {
                reply("Both done.")
            }
            _ => reply("Waiting."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || {
            allow_models(&data, &["glm-5"]);
            run(&data, &thread, "use glm-5 for the review");
        }
    })
    .await
    .unwrap();
    let all = bodies(&app.llm).await;
    let main: Vec<&Value> = all.iter().filter(|body| !is_subagent(body)).collect();
    assert!(
        spawn_description(main[0]).contains("one of glm-5"),
        "{}",
        spawn_description(main[0])
    );
    assert!(main
        .iter()
        .any(|body| mentions(body, "subagent model 'claude-opus' is not available")));
    let models: Vec<&str> = all
        .iter()
        .filter(|body| is_subagent(body))
        .filter_map(|body| body["model"].as_str())
        .collect();
    assert!(
        models.contains(&"glm-5") && models.contains(&"test-model"),
        "{models:?}"
    );
    tokio::task::spawn_blocking(move || {
        let listed = call(&data, "list_subagents", json!({"threadId": thread}));
        let review = listed
            .as_array()
            .unwrap()
            .iter()
            .find(|agent| agent["name"] == "Review")
            .unwrap()
            .clone();
        assert_eq!(review["model"], "glm-5");
        assert!(review["ran_on"].is_null());
    })
    .await
    .unwrap();
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subagent_whose_model_fails_at_once_falls_back_to_the_chat_model() {
    let app = app(None, |body| {
        if is_subagent(body) {
            if body["model"] == "glm-5" {
                return ResponseTemplate::new(400)
                    .set_body_json(json!({"error": {"message": "model glm-5 is over its quota"}}));
            }
            return reply("Done on the fallback.");
        }
        match tool_results(body) {
            0 => tools(&[("s1", "spawn_subagents", json!({"subagents": [{"write_paths": [], "name": "Review", "brief": "Review src/.", "model": "glm-5"}]}))]),
            _ if mentions(body, NOTE) => reply("Reported."),
            _ => reply("Waiting."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        allow_models(&data, &["glm-5"]);
        run(&data, &thread, "use glm-5 for the review");
        let listed = call(&data, "list_subagents", json!({"threadId": thread}));
        assert_eq!(listed[0]["status"], "finished", "{listed}");
        assert_eq!(listed[0]["model"], "glm-5");
        assert_eq!(listed[0]["ran_on"], "test-model");
        assert!(listed[0]["result"].as_str().unwrap().contains("fallback"));
        let events = history_kinds(&data, &thread);
        let ended = events
            .iter()
            .find(|e| e["kind"] == "subagent_ended")
            .unwrap();
        assert_eq!(ended["model"], "test-model");
    })
    .await
    .unwrap();
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subagent_waiting_for_approval_does_not_hold_the_reply() {
    let app = app(None, |body| {
        if is_subagent(body) {
            return tools(&[("c1", "shell", json!({"command": "nl", "args": ["-n", "notes.txt"]}))]);
        }
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [{"write_paths": [], "name": "Lister", "brief": "Number the lines of notes.txt."}]}),
            )]),
            _ if mentions(body, NOTE) => reply("Done."),
            _ => reply("Waiting."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(
            &data,
            "set_thread_approval_mode",
            json!({"threadId": thread, "mode": "custom"}),
        );
        call(
            &data,
            "send_message",
            json!({"threadId": thread, "text": "list the files"}),
        );
        let mut pending = Value::Null;
        until("the subagent never asked", || {
            pending = call(&data, "get_pending_approvals", json!({}));
            pending.as_array().is_some_and(|list| !list.is_empty())
        });
        assert_eq!(pending[0]["subagent"]["name"], "Lister", "{pending}");
        // Its approval belongs to the subagent; the reply itself is not waiting on you.
        let tasks = call(&data, "get_thread_tasks", json!({"threadId": thread}));
        assert_eq!(tasks["run"]["awaiting_approval"], false, "{tasks}");
        assert_eq!(tasks["run"]["running"], true);
        let id = call(&data, "list_subagents", json!({"threadId": thread}))[0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        call(
            &data,
            "stop_subagent",
            json!({"threadId": thread, "subagentId": id}),
        );
        until("the run never finished", || !running(&data, &thread));
    })
    .await
    .unwrap();
    app.serving.abort();
}
