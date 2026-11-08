//! Sessions, permission and prompt hooks, notifications, real plugins, async and MCP hooks.

use super::*;

/// A session starts once, and what its hook said stays with every turn of it. It used to run on
/// every turn because that was the only way its context reached the model.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_starts_once_and_its_context_stays_for_every_turn() {
    let hooks = json!({"SessionStart": [{"hooks": [{"type": "command",
        "command": "(cat; echo) >> started.jsonl; echo SESSION_CONTEXT_MARK"}]}]});
    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    run(&app, "first").await;
    run(&app, "second").await;
    run(&app, "third").await;
    let started = lines(&app, "started.jsonl");
    assert_eq!(started.len(), 1, "{started:?}");
    assert_eq!(started[0]["hook_event_name"], "SessionStart");
    assert_eq!(started[0]["source"], "startup");
    assert_eq!(started[0]["session_id"], app.thread.as_str());
    assert_eq!(started[0]["model"], "test-model");
    assert!(started[0].get("prompt").is_none(), "{:?}", started[0]);
    let all = bodies(&app.llm).await;
    assert_eq!(all.len(), 3);
    for body in &all {
        assert_eq!(
            body.to_string().matches("SESSION_CONTEXT_MARK").count(),
            1,
            "{body}"
        );
    }
    app.serving.abort();
}

/// A hook turned on partway through a chat starts on the next turn, and only that hook does.
#[tokio::test(flavor = "multi_thread")]
async fn a_hook_turned_on_in_a_running_session_starts_on_the_next_turn() {
    let hooks = json!({"SessionStart": [
        {"hooks": [{"type": "command", "command": "(cat; echo) >> one.jsonl"}]},
        {"hooks": [{"type": "command", "command": "(cat; echo) >> two.jsonl"}]},
    ]});
    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    let two = hook_named(&app, "two.jsonl");
    themis(app.data.path(), &["hook", "disable", "guarded", &two]);
    run(&app, "first").await;
    assert_eq!(lines(&app, "one.jsonl").len(), 1);
    assert!(lines(&app, "two.jsonl").is_empty());
    themis(app.data.path(), &["hook", "enable", "guarded", &two]);
    run(&app, "second").await;
    assert_eq!(lines(&app, "one.jsonl").len(), 1, "one started already");
    assert_eq!(lines(&app, "two.jsonl").len(), 1);
    assert_eq!(lines(&app, "two.jsonl")[0]["source"], "startup");
    app.serving.abort();
}

/// `SessionEnd` runs when the chat is deleted, once, with the transcript still there, and never
/// at the end of a turn.
#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_chat_ends_its_session_once_and_a_turn_never_does() {
    const ENDS: &str = r#"#!/bin/sh
input=$(cat)
printf '%s\n' "$input" >> ended.jsonl
path=$(printf '%s' "$input" | sed 's/.*"transcript_path":"\([^"]*\)".*/\1/')
grep -c first "$path" > transcript-had.txt
"#;
    let hooks = json!({"SessionEnd": [{"hooks": [{"type": "command",
        "command": "sh \"${CLAUDE_PLUGIN_ROOT}/hooks/ends.sh\""}]}]});
    let app = crate::app(hooks, &[("hooks/ends.sh", ENDS)], |_| reply("Done.")).await;
    run(&app, "first").await;
    run(&app, "second").await;
    assert!(
        lines(&app, "ended.jsonl").is_empty(),
        "a turn isn't a session"
    );
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(&data, "discard_thread", json!({"threadId": thread}));
    })
    .await
    .unwrap();
    until("SessionEnd never ran", || {
        exists(&app, "transcript-had.txt")
    });
    let ended = lines(&app, "ended.jsonl");
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0]["hook_event_name"], "SessionEnd");
    assert_eq!(ended[0]["reason"], "other");
    assert_eq!(ended[0]["themis_reason"], "chat_deleted");
    assert_eq!(ended[0]["session_id"], app.thread.as_str());
    assert!(ended[0].get("prompt").is_none());
    assert_ne!(
        std::fs::read_to_string(app.project.path().join("transcript-had.txt"))
            .unwrap()
            .trim(),
        "0"
    );
    let dir = app.data.path().join("hook-sessions").join(&app.thread);
    until("the chat's hook folder stayed", || !dir.exists());
    app.serving.abort();
}

/// Quitting ends every open session within a short budget: a slow hook is cut off rather than
/// holding the app.
#[tokio::test(flavor = "multi_thread")]
async fn quitting_ends_open_sessions_within_a_short_budget() {
    let hooks = json!({"SessionEnd": [
        {"hooks": [{"type": "command", "command": logging("ended.jsonl")}]},
        {"hooks": [{"type": "command", "command": "sleep 58; touch slow-finished", "timeout": 60}]},
    ]});
    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    run(&app, "first").await;
    let started = Instant::now();
    let data = app.data.path().to_owned();
    tokio::task::spawn_blocking(move || call(&data, "shutdown", json!({})))
        .await
        .unwrap();
    app.serving.await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    let ended = lines_in(app.project.path(), "ended.jsonl");
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert_eq!(ended[0]["reason"], "prompt_input_exit");
    assert_eq!(ended[0]["themis_reason"], "app_quit");
    assert!(!app.project.path().join("slow-finished").exists());
}

/// A chat left alone ends its session after `hooks.session_idle_minutes`; writing again starts
/// the next one, as a resume. With 0 it never ends for being idle.
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_chat_ends_its_session_and_the_next_turn_resumes() {
    let hooks = json!({
        "SessionStart": [{"hooks": [{"type": "command", "command": logging("started.jsonl")}]}],
        "SessionEnd": [{"hooks": [{"type": "command", "command": logging("ended.jsonl")}]}],
    });
    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    std::fs::create_dir_all(app.project.path().join(".themis")).unwrap();
    std::fs::write(
        app.project.path().join(".themis/config.jsonc"),
        r#"{"hooks": {"session_idle_minutes": 5}}"#,
    )
    .unwrap();
    run(&app, "first").await;
    let soon = Instant::now() + Duration::from_secs(60);
    app.state.end_idle_hook_sessions_at(soon).await;
    assert!(
        lines(&app, "ended.jsonl").is_empty(),
        "not idle long enough"
    );
    let later = Instant::now() + Duration::from_secs(10 * 60);
    app.state.end_idle_hook_sessions_at(later).await;
    assert_eq!(lines(&app, "ended.jsonl").len(), 1);
    app.state.end_idle_hook_sessions_at(later).await;
    let ended = lines(&app, "ended.jsonl");
    assert_eq!(ended.len(), 1, "once a session");
    assert_eq!(ended[0]["reason"], "other");
    assert_eq!(ended[0]["themis_reason"], "idle");
    run(&app, "second").await;
    let started = lines(&app, "started.jsonl");
    assert_eq!(started.len(), 2, "{started:?}");
    assert_eq!(started[1]["source"], "resume");
    assert_eq!(started[1]["session_id"], started[0]["session_id"]);
    std::fs::write(
        app.project.path().join(".themis/config.jsonc"),
        r#"{"hooks": {"session_idle_minutes": 0}}"#,
    )
    .unwrap();
    run(&app, "third").await;
    let much_later = Instant::now() + Duration::from_secs(100 * 60 * 60);
    app.state.end_idle_hook_sessions_at(much_later).await;
    assert_eq!(lines(&app, "ended.jsonl").len(), 1, "0 means never");
    app.serving.abort();
}

/// With nobody there to ask, a session's end runs only what was allowed when it opened: a deny
/// rule stops it, in YOLO too, and so does turning it off afterwards.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_end_hook_keeps_to_deny_rules_and_its_switch() {
    let hooks = logs("SessionEnd", "ended.jsonl");
    let later = Instant::now() + Duration::from_secs(100 * 60 * 60);
    let app = crate::app(hooks.clone(), &[], |_| reply("Done.")).await;
    run(&app, "first").await;
    let name = hook_named(&app, "ended.jsonl");
    themis(app.data.path(), &["hook", "disable", "guarded", &name]);
    app.state.end_idle_hook_sessions_at(later).await;
    assert!(lines(&app, "ended.jsonl").is_empty(), "turned off since");
    app.serving.abort();

    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    std::fs::create_dir_all(app.project.path().join(".themis")).unwrap();
    std::fs::write(
        app.project.path().join(".themis/config.jsonc"),
        r#"{"approval":{"rules":[{"tool":"hook_*","action":"deny"}]}}"#,
    )
    .unwrap();
    run(&app, "first").await;
    app.state.end_idle_hook_sessions_at(later).await;
    assert!(lines(&app, "ended.jsonl").is_empty(), "a deny rule holds");
    app.serving.abort();
}

/// In Custom a session's end is asked about once, on the turn that opens the session, so that it
/// can run later with nobody there. A person's no holds, and it isn't asked again.
#[tokio::test(flavor = "multi_thread")]
async fn in_custom_a_session_end_is_asked_about_when_the_session_opens() {
    let later = Instant::now() + Duration::from_secs(100 * 60 * 60);
    for (answer, runs) in [("once", 1), ("deny", 0)] {
        let hooks = logs("SessionEnd", "ended.jsonl");
        let app = crate::app(hooks, &[], |_| reply("Done.")).await;
        let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
        let summaries = tokio::task::spawn_blocking(move || {
            call(
                &data,
                "set_thread_approval_mode",
                json!({"threadId": thread, "mode": "custom"}),
            );
            let mut summaries = Vec::new();
            for text in ["first", "second"] {
                call(
                    &data,
                    "send_message",
                    json!({"threadId": thread, "text": text}),
                );
                let deadline = Instant::now() + Duration::from_secs(30);
                while call(&data, "get_thread", json!({"threadId": thread}))["running"] == true {
                    assert!(Instant::now() < deadline, "the run never finished");
                    let pending = call(&data, "get_pending_approvals", json!({}));
                    for approval in pending.as_array().into_iter().flatten() {
                        summaries.push(approval["summary"].as_str().unwrap_or_default().to_owned());
                        call(
                            &data,
                            "approve_action",
                            json!({"threadId": thread,
                                "approvalId": approval["approval_id"], "decision": answer}),
                        );
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            summaries
        })
        .await
        .unwrap();
        assert_eq!(summaries.len(), 1, "{summaries:?}");
        assert!(summaries[0].starts_with("SessionEnd:"), "{summaries:?}");
        app.state.end_idle_hook_sessions_at(later).await;
        assert_eq!(lines(&app, "ended.jsonl").len(), runs, "{answer}");
        app.serving.abort();
    }
}

/// Some hooks hand their work to a detached process and exit at once, as Claude's short budget
/// makes them. That process must live on.
#[tokio::test(flavor = "multi_thread")]
async fn a_process_a_hook_detaches_outlives_the_hook() {
    let hooks = json!({"PostToolUse": [{"hooks": [{"type": "command",
        "command": "nohup sh -c 'sleep 1; touch detached-finished' >/dev/null 2>&1 </dev/null &"}]}]});
    let app = crate::app(hooks, &[], calls_once("shell", json!({"command": "ls"}))).await;
    run(&app, "go").await;
    until("the detached process was killed with its hook", || {
        exists(&app, "detached-finished")
    });
    app.serving.abort();
}

/// `/skill` in the person's message is expanded with hooks told which, on what they wrote; what
/// they add reaches the model, and a matcher takes the skill by name.
#[tokio::test(flavor = "multi_thread")]
async fn a_written_skill_runs_the_expansion_hooks() {
    let hooks = json!({"UserPromptExpansion": [
        {"matcher": "^guarded:review$", "hooks": [{"type": "command",
            "command": "(cat; echo) >> expanded.jsonl; echo EXPANSION_MARK"}]},
        {"matcher": "other", "hooks": [{"type": "command", "command": "touch wrong-skill"}]},
    ]});
    let app = crate::app(hooks, &[("skills/review/SKILL.md", SKILL_FILE)], |_| {
        reply("Done.")
    })
    .await;
    run(&app, "plain message").await;
    assert!(lines(&app, "expanded.jsonl").is_empty(), "nothing expanded");
    run(&app, "[[skill:g--guarded--review]] the parser").await;
    let seen = lines(&app, "expanded.jsonl");
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0]["hook_event_name"], "UserPromptExpansion");
    assert_eq!(seen[0]["expansion_type"], "slash_command");
    assert_eq!(seen[0]["command_name"], "guarded:review");
    assert_eq!(seen[0]["command_args"], "the parser");
    assert_eq!(seen[0]["command_source"], "plugin");
    assert!(!exists(&app, "wrong-skill"));
    assert!(bodies(&app.llm).await[1]
        .to_string()
        .contains("EXPANSION_MARK"));
    app.serving.abort();
    let hooks = json!({"UserPromptExpansion": [{"hooks": [{"type": "command",
        "command": "echo 'that command is off' >&2; exit 2"}]}]});
    let app = crate::app(hooks, &[("skills/review/SKILL.md", SKILL_FILE)], |_| {
        reply("Done.")
    })
    .await;
    run(&app, "[[skill:g--guarded--review]] please").await;
    assert!(
        bodies(&app.llm).await.is_empty(),
        "a refused expansion never reaches the model"
    );
    app.serving.abort();
}

/// After a step's tool calls and before the next request, hooks hear about the whole batch once;
/// what they add reaches that next request, labelled as the plugin's.
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_of_tool_calls_runs_its_hook_once_and_adds_context() {
    let hooks = json!({"PostToolBatch": [{"hooks": [{"type": "command", "command":
        "(cat; echo) >> batch.jsonl; echo '{\"hookSpecificOutput\":{\"additionalContext\":\"BATCH_MARK\"}}'"}]}]});
    let app = crate::app(hooks, &[], |body| {
        if tool_results(body).is_empty() {
            ResponseTemplate::new(200).set_body_json(json!({"choices": [{"message": {
                "role": "assistant", "content": "", "tool_calls": [
                    {"id": "a", "type": "function", "function": {"name": "shell",
                        "arguments": "{\"command\":\"ls\"}"}},
                    {"id": "b", "type": "function", "function": {"name": "read_file",
                        "arguments": "{\"file_path\":\"missing.txt\"}"}},
                ]}, "finish_reason": "tool_calls"}]}))
        } else {
            reply("Done.")
        }
    })
    .await;
    run(&app, "go").await;
    let batches = lines(&app, "batch.jsonl");
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(batches[0]["hook_event_name"], "PostToolBatch");
    let calls = batches[0]["tool_calls"].as_array().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["tool_name"], "Bash");
    assert_eq!(calls[1]["tool_name"], "Read");
    assert!(calls[0].get("tool_response").is_some());
    let all = bodies(&app.llm).await;
    assert!(!all[0].to_string().contains("BATCH_MARK"));
    assert!(all[1].to_string().contains("BATCH_MARK"));
    app.serving.abort();
    let hooks = json!({"PostToolBatch": [{"hooks": [{"type": "command",
        "command": "echo 'enough of that' >&2; exit 2"}]}]});
    let app = crate::app(hooks, &[], calls_once("shell", json!({"command": "ls"}))).await;
    run(&app, "go").await;
    assert_eq!(
        bodies(&app.llm).await.len(),
        1,
        "a refused batch stops before the next request"
    );
    app.serving.abort();
}

/// A hook runs when a question is about to be asked. Deny is honoured with no question asked;
/// allow is ignored, because hooks never grant permissions: the person is still asked.
#[tokio::test(flavor = "multi_thread")]
async fn a_permission_hook_may_deny_and_its_allow_changes_nothing() {
    const DENIES: &str = r#"(cat; echo) >> asked.jsonl; echo '{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"deny","message":"no shells today"}}}'"#;
    let touch = json!({"command": "touch", "args": ["ran.txt"]});
    let hooks = json!({
        "PermissionRequest": [{"matcher": "Bash", "hooks": [{"type": "command", "command": DENIES}]}],
        "PermissionDenied": [{"matcher": "Bash", "hooks": [{"type": "command", "command": logging("denied.jsonl")}]}],
    });
    let app = crate::app(hooks, &[], calls_once("shell", touch.clone())).await;
    assert_eq!(
        run_custom_until_asked(&app, "go").await,
        None,
        "nobody asked"
    );
    until("the run never ended", || {
        !lines(&app, "denied.jsonl").is_empty()
    });
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        until("the run never finished", || {
            call(&data, "get_thread", json!({"threadId": thread}))["running"] != true
        });
    })
    .await
    .unwrap();
    let asked = lines(&app, "asked.jsonl");
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["hook_event_name"], "PermissionRequest");
    assert_eq!(asked[0]["tool_name"], "Bash");
    assert_eq!(asked[0]["tool_input"]["command"], "touch ran.txt");
    assert_eq!(lines(&app, "denied.jsonl")[0]["reason"], "no shells today");
    assert!(!exists(&app, "ran.txt"));
    assert!(
        last_tool_result(&app).await.contains("no shells today"),
        "the agent hears why"
    );
    app.serving.abort();

    // An answer of allow, even asking to change the command: nothing runs until the person says.
    const ALLOWS: &str = r#"echo '{"hookSpecificOutput":{"decision":{"behavior":"allow","updatedInput":{"command":"touch hook-chose.txt"}}}}'"#;
    let hooks = json!({
        "PermissionRequest": [{"hooks": [{"type": "command", "command": ALLOWS}]}],
        "PermissionDenied": [{"hooks": [{"type": "command", "command": logging("denied.jsonl")}]}],
        "Notification": [{"matcher": "permission_prompt", "hooks": [{"type": "command", "command": logging("told.jsonl")}]}],
    });
    let app = crate::app(hooks, &[], calls_once("shell", touch)).await;
    let id = run_custom_until_asked(&app, "go")
        .await
        .expect("the person wasn't asked");
    assert!(!exists(&app, "ran.txt"));
    assert!(!exists(&app, "hook-chose.txt"));
    let told = lines(&app, "told.jsonl");
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0]["notification_type"], "permission_prompt");
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(
            &data,
            "approve_action",
            json!({"threadId": thread, "approvalId": id, "decision": "deny"}),
        );
        until("the run never finished", || {
            call(&data, "get_thread", json!({"threadId": thread}))["running"] != true
        });
    })
    .await
    .unwrap();
    assert!(!exists(&app, "ran.txt"));
    assert!(!exists(&app, "hook-chose.txt"));
    let denied = lines(&app, "denied.jsonl");
    assert_eq!(denied.len(), 1, "the person's no: {denied:?}");
    assert_eq!(denied[0]["tool_name"], "Bash");
    app.serving.abort();
}

/// Between turns of a session, a configuration file that changed is reported on the turn that
/// picks it up; instructions count as loaded once a session.
#[tokio::test(flavor = "multi_thread")]
async fn a_changed_configuration_and_loaded_instructions_reach_their_hooks() {
    let hooks = json!({
        "ConfigChange": [{"matcher": "project_settings", "hooks": [{"type": "command", "command": logging("changed.jsonl")}]}],
        "InstructionsLoaded": [{"hooks": [{"type": "command", "command": logging("loaded.jsonl")}]}],
    });
    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    std::fs::write(app.project.path().join("RULES.md"), "Keep it short.").unwrap();
    std::fs::create_dir_all(app.project.path().join(".themis")).unwrap();
    let config = app.project.path().join(".themis/config.jsonc");
    std::fs::write(&config, r#"{"instructions": ["RULES.md"]}"#).unwrap();
    run(&app, "first").await;
    run(&app, "second").await;
    assert!(lines(&app, "changed.jsonl").is_empty(), "nothing changed");
    let loaded = lines(&app, "loaded.jsonl");
    assert_eq!(loaded.len(), 1, "{loaded:?}");
    assert_eq!(loaded[0]["load_reason"], "session_start");
    assert!(loaded[0]["file_path"]
        .as_str()
        .unwrap()
        .ends_with("RULES.md"));
    std::fs::write(
        &config,
        r#"{"instructions": ["RULES.md"], "hooks": {"max_wakes": 1}}"#,
    )
    .unwrap();
    run(&app, "third").await;
    let changed = lines(&app, "changed.jsonl");
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0]["source"], "project_settings");
    assert!(changed[0]["file_path"]
        .as_str()
        .unwrap()
        .ends_with("config.jsonc"));
    run(&app, "fourth").await;
    assert_eq!(lines(&app, "changed.jsonl").len(), 1, "once for one change");
    app.serving.abort();
}

/// A prompt hook asks the chat's model, with no tools, what the plugin wants checked. A guard's no
/// refuses the call and says why; its plugin's text and the input reach only that request.
#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_hook_asks_the_model_and_its_no_refuses_the_call() {
    let hooks = prompt_hook("PreToolUse", "Write", "Refuse writes of $ARGUMENTS please");
    let app = crate::app(hooks, &[], |body| {
        if is_prompt_hook(body) {
            return reply(r#"{"ok": false, "reason": "not in this folder"}"#);
        }
        if tool_results(body).is_empty() {
            tool_call("c1", "write_file", &writes())
        } else {
            reply("Done.")
        }
    })
    .await;
    run(&app, "write it").await;
    assert!(!exists(&app, "new.txt"), "the guard said no");
    assert!(last_tool_result(&app).await.contains("not in this folder"));
    let all = bodies(&app.llm).await;
    let asked = all.iter().find(|body| is_prompt_hook(body)).unwrap();
    assert_eq!(asked["model"], "test-model", "the chat's model");
    assert!(
        asked.get("tools").is_none_or(Value::is_null),
        "read-only: {asked}"
    );
    let text = asked.to_string();
    assert!(text.contains("Refuse writes of"), "{text}");
    assert!(
        text.contains("THE_FILE_TEXT"),
        "$ARGUMENTS is the input: {text}"
    );
    assert!(
        !text.contains("write it"),
        "not the person's message: {text}"
    );
    assert!(!all
        .iter()
        .filter(|body| !is_prompt_hook(body))
        .any(|body| body.to_string().contains("Refuse writes of")));
    app.serving.abort();
}

/// A guard that can't get a clear yes holds: an answer that can't be read, a request that fails, or
/// a budget that's gone all refuse the call.
#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_guard_fails_closed() {
    for (answer, status) in [("I am not sure about this one.", 200), ("{}", 400)] {
        let hooks = prompt_hook("PreToolUse", "Write", "Check it");
        let app = crate::app(hooks, &[], move |body| {
            if is_prompt_hook(body) {
                return if status == 200 {
                    reply(answer)
                } else {
                    ResponseTemplate::new(status).set_body_string(answer)
                };
            }
            if tool_results(body).is_empty() {
                tool_call("c1", "write_file", &writes())
            } else {
                reply("Done.")
            }
        })
        .await;
        run(&app, "write it").await;
        assert!(!exists(&app, "new.txt"), "{answer}");
        app.serving.abort();
    }
    // One call a run: the first write is checked and passes, the second finds nothing left.
    let hooks = prompt_hook("PreToolUse", "Write", "Check it");
    let app = crate::app(hooks, &[], |body| {
        if is_prompt_hook(body) {
            return reply(r#"{"ok": true}"#);
        }
        match tool_results(body).len() {
            0 => tool_call("c1", "write_file", &writes()),
            1 => tool_call(
                "c2",
                "write_file",
                &json!({"file_path": "second.txt", "content": "x"}),
            ),
            _ => reply("Done."),
        }
    })
    .await;
    std::fs::write(
        app.data.path().join("runtime.jsonc"),
        r#"{"hooks": {"prompt_hook_calls": 1}}"#,
    )
    .unwrap();
    run(&app, "write both").await;
    assert!(exists(&app, "new.txt"), "a yes lets it through");
    assert!(!exists(&app, "second.txt"), "no budget, no write");
    assert_eq!(
        bodies(&app.llm)
            .await
            .iter()
            .filter(|body| is_prompt_hook(body))
            .count(),
        1
    );
    app.serving.abort();
}

/// A hook whose plugin says it must never block (paypal's) doesn't hold when it gets no answer, or
/// when the run's checks are used up. Its plain no still refuses, and any other guard still holds.
#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_hook_that_says_it_never_blocks_lets_the_call_through_on_no_answer() {
    const NEVER: &str = "This hook is informational only and must never block or deny an edit.";
    let hooks = prompt_hook("PreToolUse", "Write", NEVER);
    let app = crate::app(hooks, &[], |body| {
        if is_prompt_hook(body) {
            return ResponseTemplate::new(400).set_body_string("{}");
        }
        if tool_results(body).is_empty() {
            tool_call("c1", "write_file", &writes())
        } else {
            reply("Done.")
        }
    })
    .await;
    run(&app, "write it").await;
    assert!(exists(&app, "new.txt"), "a failed request held it");
    app.serving.abort();
    // Used up.
    let hooks = prompt_hook("PreToolUse", "Write", NEVER);
    let app = crate::app(hooks, &[], |body| {
        if is_prompt_hook(body) {
            return reply(r#"{"ok": true}"#);
        }
        match tool_results(body).len() {
            0 => tool_call("c1", "write_file", &writes()),
            1 => tool_call(
                "c2",
                "write_file",
                &json!({"file_path": "second.txt", "content": "x"}),
            ),
            _ => reply("Done."),
        }
    })
    .await;
    std::fs::write(
        app.data.path().join("runtime.jsonc"),
        r#"{"hooks": {"prompt_hook_calls": 1}}"#,
    )
    .unwrap();
    run(&app, "write both").await;
    assert!(exists(&app, "second.txt"), "a used-up budget held it");
    app.serving.abort();
    // A clear no is still a no.
    let hooks = prompt_hook("PreToolUse", "Write", NEVER);
    let app = crate::app(hooks, &[], |body| {
        if is_prompt_hook(body) {
            return reply(r#"{"ok": false, "reason": "no"}"#);
        }
        if tool_results(body).is_empty() {
            tool_call("c1", "write_file", &writes())
        } else {
            reply("Done.")
        }
    })
    .await;
    run(&app, "write it").await;
    assert!(!exists(&app, "new.txt"), "its no didn't refuse");
    app.serving.abort();
}

/// `hooks.prompt_hook_calls: 0` is the person turning these off: nothing is asked and nothing is
/// refused for it. A project can't do that to them.
#[tokio::test(flavor = "multi_thread")]
async fn prompt_hooks_can_be_turned_off_by_the_person_only() {
    let hooks = prompt_hook("PreToolUse", "Write", "Check it");
    let app = crate::app(hooks.clone(), &[], |body| {
        if is_prompt_hook(body) {
            return reply(r#"{"ok": false, "reason": "no"}"#);
        }
        if tool_results(body).is_empty() {
            tool_call("c1", "write_file", &writes())
        } else {
            reply("Done.")
        }
    })
    .await;
    std::fs::write(
        app.data.path().join("runtime.jsonc"),
        r#"{"hooks": {"prompt_hook_calls": 0}}"#,
    )
    .unwrap();
    run(&app, "write it").await;
    assert!(exists(&app, "new.txt"));
    assert!(!bodies(&app.llm).await.iter().any(is_prompt_hook));
    app.serving.abort();

    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    std::fs::create_dir_all(app.project.path().join(".themis")).unwrap();
    std::fs::write(
        app.project.path().join(".themis/config.jsonc"),
        r#"{"hooks": {"prompt_hook_calls": 0}}"#,
    )
    .unwrap();
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    let refused = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
            .arg("--data-dir")
            .arg(&data)
            .args([
                "call",
                "send_message",
                &json!({"threadId": thread, "text": "hi"}).to_string(),
            ])
            .env_remove("OPENCODE_KEY")
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("hooks.prompt_hook_calls"));
    app.serving.abort();
}

/// After a call has run, a no can't undo it: the agent hears the plugin's reason, labelled, and
/// goes on. A yes says nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_hook_after_a_tool_tells_the_agent() {
    let hooks = prompt_hook("PostToolUse", "Bash", "Was the listing as expected?");
    let app = crate::app(hooks, &[], |body| {
        if is_prompt_hook(body) {
            return reply(r#"{"ok": false, "reason": "Confirm the row count"}"#);
        }
        if tool_results(body).is_empty() {
            tool_call("c1", "shell", &json!({"command": "ls"}))
        } else {
            reply("Done.")
        }
    })
    .await;
    run(&app, "look").await;
    let all = bodies(&app.llm).await;
    let last = all
        .iter()
        .rev()
        .find(|body| !is_prompt_hook(body))
        .unwrap()
        .to_string();
    assert!(last.contains("Confirm the row count"), "{last}");
    assert!(last.contains("not from the person"), "{last}");
    app.serving.abort();
}

/// What the desktop notified about, for hooks that want to hear, whether or not a run is going.
#[tokio::test(flavor = "multi_thread")]
async fn themis_notifying_runs_the_notification_hooks() {
    let hooks = json!({"Notification": [{"matcher": "goal_reached", "hooks": [{"type": "command",
        "command": logging("told.jsonl")}]}]});
    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    run(&app, "first").await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        for (kind, outcome) in [
            ("blocked", "sent"),
            ("reached", "denied"),
            ("reached", "sent"),
        ] {
            call(
                &data,
                "record_notification",
                json!({"record": {"thread_id": thread, "kind": kind, "outcome": outcome}}),
            );
        }
    })
    .await
    .unwrap();
    until("the hook never heard", || {
        !lines(&app, "told.jsonl").is_empty()
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let told = lines(&app, "told.jsonl");
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0]["notification_type"], "goal_reached");
    assert_eq!(told[0]["hook_event_name"], "Notification");
    app.serving.abort();
}

/// A chat continued after a restart is what a `resume` hook runs on, and only then.
#[tokio::test(flavor = "multi_thread")]
async fn a_resume_hook_runs_on_a_chats_first_turn_after_a_restart() {
    let hooks = json!({"SessionStart": [{"matcher": "resume", "hooks": [{"type": "command",
        "command": "cat >> resumed.jsonl; echo >> resumed.jsonl"}]}]});
    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    run(&app, "first").await;
    assert!(
        !exists(&app, "resumed.jsonl"),
        "a new chat isn't a resumed one"
    );
    run(&app, "second").await;
    assert!(!exists(&app, "resumed.jsonl"), "nor is its next turn");
    app.serving.abort();
    tokio::time::sleep(Duration::from_millis(200)).await;
    // The app starts again on the same data.
    let state = AppState::new_for_test(app.data.path().join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(app.llm.uri()));
    let server = Server::bind(state, app.data.path()).await.unwrap();
    let serving = tokio::spawn(async move {
        let _ = server.run().await;
    });
    run(&app, "third").await;
    let first = std::fs::read_to_string(app.project.path().join("resumed.jsonl")).unwrap();
    assert!(first.contains("\"source\":\"resume\""), "{first}");
    run(&app, "fourth").await;
    let later = std::fs::read_to_string(app.project.path().join("resumed.jsonl")).unwrap();
    assert_eq!(first, later, "it runs once");
    serving.abort();
}

/// The real `remember` plugin through a session: its `SessionEnd` hook exits at once and its
/// detached flush goes on to write memory, reading the transcript Themis keeps. It isn't in this
/// repository and its summarizer is `claude -p`, so this is opt-in. To keep it off any account, run
/// it with a stand-in `claude` first on `PATH` and a throwaway `HOME`:
///
/// ```sh
/// HOME=/tmp/empty PATH=/tmp/fake-claude:$PATH THEMIS_REMEMBER_PLUGIN=/path/to/remember \
///   cargo test -p themis-desktop --test plugin_hooks_cli_e2e -- --ignored the_real_remember --nocapture
/// ```
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a copy of the remember plugin and a stand-in claude"]
async fn the_real_remember_flushes_when_a_session_ends() {
    let real = std::path::PathBuf::from(std::env::var_os("THEMIS_REMEMBER_PLUGIN").unwrap());
    let app = real_plugin(real).await;
    run(&app, "Discuss the parser design").await;
    run(&app, "And the lexer").await;
    let memory = app.project.path().join(".remember");
    let tree = |dir: &Path| {
        let output = std::process::Command::new("find")
            .arg(dir)
            .arg("-type")
            .arg("f")
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    println!("before the end:\n{}", tree(&memory));
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(&data, "discard_thread", json!({"threadId": thread}));
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_secs(25)).await;
    println!("after the end:\n{}", tree(&memory));
    let text = |name: &str| std::fs::read_to_string(memory.join(name)).unwrap_or_default();
    for name in ["logs/hook-errors.log", "now.md"] {
        println!("--- {name}\n{}", text(name));
    }
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg("tail -n 40 .remember/logs/*.log")
        .current_dir(app.project.path())
        .output()
        .unwrap();
    println!("{}", String::from_utf8_lossy(&output.stdout));
    let log = String::from_utf8_lossy(&output.stdout);
    assert!(
        log.contains("SessionEnd") || log.contains("session-end"),
        "{log}"
    );
    app.serving.abort();
}

/// The real `vercel` plugin through a session: its session state stays across turns and is cleaned
/// up once, when the session ends. Opt-in, as it isn't in this repository:
///
/// ```sh
/// HOME=/tmp/empty TMPDIR=/tmp/empty THEMIS_VERCEL_PLUGIN=/path/to/vercel cargo test -p themis-desktop \
///   --test plugin_hooks_cli_e2e -- --ignored the_real_vercel --nocapture
/// ```
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a copy of the vercel plugin"]
async fn the_real_vercel_keeps_its_state_until_the_session_ends() {
    let real = std::path::PathBuf::from(std::env::var_os("THEMIS_VERCEL_PLUGIN").unwrap());
    let app = real_plugin(real).await;
    let temp = std::env::temp_dir();
    let prefix = format!("vercel-plugin-{}-", app.thread);
    // The kind of thing its hooks leave as they go, as they name it.
    let state = temp.join(format!("{prefix}seen-skills.txt"));
    run(&app, "Deploy my nextjs app").await;
    std::fs::write(&state, "nextjs").unwrap();
    run(&app, "And again").await;
    assert_eq!(
        std::fs::read_to_string(&state).unwrap(),
        "nextjs",
        "a turn doesn't reset the session"
    );
    let env_files: Vec<_> =
        std::fs::read_dir(app.data.path().join("hook-sessions").join(&app.thread))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
    println!("session folder: {env_files:?}");
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(&data, "discard_thread", json!({"threadId": thread}));
    })
    .await
    .unwrap();
    until("its session state stayed", || !state.exists());
    let all = bodies(&app.llm).await;
    println!(
        "first request's hook context: {}",
        all[0].to_string().contains("hook-context")
    );
    app.serving.abort();
}

/// The real `ralph-loop` stop hook, from a copy of the plugin. It isn't in this repository, so
/// this is opt-in:
///
/// ```sh
/// THEMIS_RALPH_PLUGIN=/path/to/plugins/ralph-loop cargo test -p themis-desktop \\
///   --test plugin_hooks_cli_e2e -- --ignored the_real_ralph_loop --nocapture
/// ```
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a copy of the ralph-loop plugin"]
async fn the_real_ralph_loop_goes_round_and_ends_on_its_promise() {
    let real = std::path::PathBuf::from(std::env::var_os("THEMIS_RALPH_PLUGIN").unwrap());
    let script = std::fs::read_to_string(real.join("hooks/stop-hook.sh")).unwrap();
    let hooks: Value =
        serde_json::from_str(&std::fs::read_to_string(real.join("hooks/hooks.json")).unwrap())
            .unwrap();
    let app = crate::app(
        hooks["hooks"].clone(),
        &[("hooks/stop-hook.sh", &script)],
        |body| {
            let rounds = body.to_string().matches("Polish the notes").count();
            // Once in the state file's prompt, handed back each round.
            match rounds {
                0 | 1 => reply("Round one."),
                2 => reply("Round two."),
                _ => reply("All done. <promise>FINISHED</promise>"),
            }
        },
    )
    .await;
    let state = app.project.path().join(".claude/ralph-loop.local.md");
    std::fs::create_dir_all(state.parent().unwrap()).unwrap();
    std::fs::write(
        &state,
        "---\nactive: true\niteration: 1\nmax_iterations: 8\ncompletion_promise: \"FINISHED\"\nstarted_at: \"x\"\n---\n\nPolish the notes\n",
    )
    .unwrap();
    run(&app, "go").await;
    let all = bodies(&app.llm).await;
    assert!(all.len() >= 3, "it never looped: {}", all.len());
    assert!(!state.exists(), "the promise didn't end the loop");
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_async_hook_does_not_hold_the_turn_and_cannot_block() {
    let app = app(
        json!({"PreToolUse": [{"matcher": "Bash", "hooks": [{
            "type": "command",
            "command": "sleep 3; touch late; echo '{\"decision\":\"block\",\"reason\":\"no\"}'",
            "async": true
        }]}]}),
        &[],
        calls_once("shell", json!({"command": "touch", "args": ["ran"]})),
    )
    .await;
    let started = Instant::now();
    run(&app, "go").await;
    assert!(exists(&app, "ran"), "an async hook can't block the call");
    assert!(
        !exists(&app, "late"),
        "the turn waited {:?} for the async hook",
        started.elapsed()
    );
    let project = app.project.path().to_owned();
    tokio::task::spawn_blocking(move || {
        until("the async hook never finished", || {
            project.join("late").exists()
        });
    })
    .await
    .unwrap();
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_mcp_matcher_guards_the_tools_it_names_and_only_those() {
    let mcp = mcp_server().await;
    let file = json!({"mcpServers": {"stats": {"type": "http", "url": mcp.uri()}}}).to_string();
    let hooks = json!({"PreToolUse": [{"matcher": "mcp__.*__run_query", "hooks": [{
        "type": "command",
        "command": "cat > seen.json; echo 'query refused' >&2; exit 2"
    }]}]});
    let responder = |tail: &'static str| {
        move |body: &Value| {
            if tool_results(body).is_empty() {
                let name = mcp_wire_name(body, tail).expect("the tool");
                tool_call("c1", &name, &json!({}))
            } else {
                reply("Done.")
            }
        }
    };
    // The guard refuses the tool it names; the server never sees the call.
    let guarded = app(
        hooks.clone(),
        &[(".mcp.json", &file)],
        responder("_run_query"),
    )
    .await;
    run(&guarded, "query").await;
    let result = last_tool_result(&guarded).await;
    assert!(result.contains("query refused"), "{result}");
    assert!(!called(&mcp, "run_query").await, "the guard let it through");
    let seen: Value =
        serde_json::from_slice(&std::fs::read(guarded.project.path().join("seen.json")).unwrap())
            .unwrap();
    assert_eq!(seen["tool_name"], "mcp__stats__run_query", "{seen}");
    guarded.serving.abort();
    // Another tool of the same server passes.
    let free = app(hooks, &[(".mcp.json", &file)], responder("_other")).await;
    run(&free, "other").await;
    assert!(called(&mcp, "other").await);
    assert!(!free.project.path().join("seen.json").exists());
    free.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_failure_runs_only_when_the_run_failed() {
    let hooks = json!({"StopFailure": [{"hooks": [
        {"type": "command", "command": "touch failed-hook"}
    ]}]});
    let good = app(hooks.clone(), &[], |_| reply("Fine.")).await;
    run(&good, "hello").await;
    assert!(!exists(&good, "failed-hook"), "it ran on a good turn");
    good.serving.abort();

    let bad = app(hooks, &[], |_| {
        ResponseTemplate::new(401).set_body_string("no")
    })
    .await;
    run(&bad, "hello").await;
    let project = bad.project.path().to_owned();
    tokio::task::spawn_blocking(move || {
        until("the hook never ran on a failed run", || {
            project.join("failed-hook").exists()
        });
    })
    .await
    .unwrap();
    bad.serving.abort();
}

/// In Custom a new chat's `SessionStart` hook is asked about before the model is, and the box
/// reaches a subscribed window at once, as a tool's does.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_start_question_reaches_a_subscribed_window_at_once() {
    let hooks = logs("SessionStart", "started.jsonl");
    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    let client = themis_desktop::server::Client::new(data.clone());
    client
        .call(
            "set_thread_approval_mode",
            json!({"threadId": thread, "mode": "custom"}),
        )
        .await
        .unwrap();
    let mut events = client.subscribe().await.unwrap();
    let sent = Instant::now();
    client
        .call("send_message", json!({"threadId": thread, "text": "first"}))
        .await
        .unwrap();
    let request = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = themis_desktop::server::Client::next_event(&mut events)
                .await
                .unwrap();
            if event["name"] == "approval-request" {
                return event["payload"].clone();
            }
        }
    })
    .await
    .expect("the SessionStart question never reached the window");
    let waited = sent.elapsed();
    assert!(
        request["summary"]
            .as_str()
            .unwrap_or_default()
            .starts_with("SessionStart:"),
        "{request}"
    );
    assert!(waited < Duration::from_secs(2), "the box took {waited:?}");
    client
        .call(
            "approve_action",
            json!({"threadId": thread, "approvalId": request["approval_id"], "decision": "deny"}),
        )
        .await
        .unwrap();
    tokio::task::spawn_blocking(move || {
        until("the run never finished", || {
            call(&data, "get_thread", json!({"threadId": thread}))["running"] != true
        });
    })
    .await
    .unwrap();
    app.serving.abort();
}
