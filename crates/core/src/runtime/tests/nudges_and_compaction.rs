//! The context breakdown, run nudges, steering, deadlines and compaction.

use super::*;

/// The goal note a run carries is the meter's Goal part, only the latest one, and the parts
/// still add up to the total.
#[test]
fn the_latest_goal_note_has_its_own_part_of_the_context_breakdown() {
    let user = |content: String| ChatMessage {
        role: ChatRole::User,
        message_type: MessageType::Text,
        content,
    };
    let note = crate::goal::goal_note("ship the report", false);
    let messages = [
        user(format!("{}\n\n{note}", crate::goal::turn_message(1, ""))),
        user(format!(
            "{}{}",
            "Fix the footer too. ".repeat(20),
            crate::goal::goal_context(&note)
        )),
    ];
    let breakdown = context_breakdown(&messages);
    let first = crate::calibration::message_tokens(&messages[0]);
    let second = crate::calibration::message_tokens(&messages[1]);
    assert!(
        breakdown.goal > 0 && breakdown.goal < second,
        "{breakdown:?}"
    );
    assert!(
        breakdown.conversation >= first,
        "an earlier goal turn is conversation"
    );
    assert_eq!(
        breakdown.goal + breakdown.conversation,
        estimated_tokens(&messages)
    );
    let plain = context_breakdown(&[user("hello".into())]);
    assert_eq!(plain.goal, 0);
}

#[tokio::test]
async fn the_memory_block_is_its_own_part_taken_out_of_the_system_part() {
    let block = format!(
        "Memory from earlier chats\n{}",
        "- a (user, 2026-11-03): x\n".repeat(40)
    );
    let system = ChatMessage {
        role: ChatRole::System,
        message_type: MessageType::Text,
        content: format!("Instructions.\n{block}\nMore instructions."),
    };
    let user = ChatMessage {
        role: ChatRole::User,
        message_type: MessageType::Text,
        content: "hello".into(),
    };
    let messages = [system, user];
    let plain = context_breakdown(&messages);
    assert_eq!(plain.memory, 0);
    let split = with_memory_block(block.clone(), async { context_breakdown(&messages) }).await;
    assert_eq!(split.memory, block.len().div_ceil(4));
    assert_eq!(
        split.memory + split.system,
        plain.system,
        "the parts still add up"
    );
    assert_eq!(split.conversation, plain.conversation);
    // A block the request doesn't carry counts nothing.
    let absent = with_memory_block("not sent".into(), async { context_breakdown(&messages) }).await;
    assert_eq!(absent.memory, 0);
    assert_eq!(absent.system, plain.system);
}

#[tokio::test]
async fn cut_off_retries_grow_the_cap_but_stay_within_a_third_of_the_budget() {
    // 200k memory: 32,768 doubles to 65,536, then stops at a third of the budget.
    let wide = MODEL_REQUEST_BUDGET
        .scope(200_000, async {
            (0..4)
                .map(|n| retry_output_cap(32_768, n))
                .collect::<Vec<_>>()
        })
        .await;
    assert_eq!(wide, [32_768, 65_536, 66_666, 66_666]);
    // 32k memory: the ordinary cap is already a third, so a retry cannot grow it (a
    // 15,109-token write let through at 16,000 left the next checkpoint no room).
    let squeezed = MODEL_REQUEST_BUDGET
        .scope(32_000, async {
            (0..4)
                .map(|n| retry_output_cap(10_666, n))
                .collect::<Vec<_>>()
        })
        .await;
    assert_eq!(squeezed, [10_666, 10_666, 10_666, 10_666]);
    // Outside a run: the old doubling, up to four times.
    assert_eq!(retry_output_cap(8_192, 3), 32_768);
}

#[tokio::test(start_paused = true)]
async fn answer_deadlines_count_only_time_without_provider_data() {
    use std::time::Duration;
    let idle = Duration::from_secs(300);
    // Streaming for 20 minutes, with data every minute: long past `idle`, but never idle.
    let progress = Arc::new(std::sync::Mutex::new(tokio::time::Instant::now()));
    let streaming = crate::providers::STREAM_PROGRESS.scope(Arc::clone(&progress), async {
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_secs(60)).await;
            crate::providers::note_stream_progress();
        }
        "done"
    });
    let started = tokio::time::Instant::now();
    let finished = unless_idle(idle, ATTEMPT_CEILING, Arc::clone(&progress), streaming).await;
    assert_eq!(finished, Ok("done"));
    assert_eq!(started.elapsed(), Duration::from_secs(1200));
    // Silent for longer than `idle`: it times out at `idle`, as before.
    let progress = Arc::new(std::sync::Mutex::new(tokio::time::Instant::now()));
    let started = tokio::time::Instant::now();
    let stalled = unless_idle(
        idle,
        ATTEMPT_CEILING,
        progress,
        std::future::pending::<()>(),
    )
    .await;
    assert_eq!(stalled, Err(()));
    assert_eq!(started.elapsed(), idle);
    // Data that never stops still ends at the ceiling.
    let progress = Arc::new(std::sync::Mutex::new(tokio::time::Instant::now()));
    let trickle = crate::providers::STREAM_PROGRESS.scope(Arc::clone(&progress), async {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            crate::providers::note_stream_progress();
        }
    });
    let started = tokio::time::Instant::now();
    assert_eq!(
        unless_idle(idle, ATTEMPT_CEILING, progress, trickle).await,
        Err(())
    );
    assert_eq!(started.elapsed(), ATTEMPT_CEILING);
}

/// The spec check: a run that changed files gets one request to verify its work against
/// the user's request before it ends; a run that changed nothing does not, and an eval
/// can switch it off.
#[cfg(feature = "eval")]
#[tokio::test]
async fn spec_check_asks_once_after_file_changes() {
    use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};
    async fn run(nudges_off: Vec<String>, write: bool) -> (String, Vec<serde_json::Value>) {
        let server = MockServer::builder().start().await;
        Mock::given(method("POST"))
                .respond_with(move |request: &Request| {
                    let body: serde_json::Value = request.body_json().unwrap();
                    let messages = body["messages"].as_array().unwrap();
                    let last = messages.last().unwrap();
                    let reply = if last["content"].as_str().is_some_and(|text| text.starts_with("[Runtime check")) {
                        serde_json::json!({"content":"Verified: a.txt holds 1."})
                    } else if write && !messages.iter().any(|message| message["role"] == "tool") {
                        serde_json::json!({"content":"Writing.","tool_calls":[{"id":"w","type":"function","function":{"name":"write_file","arguments":"{\"file_path\":\"a.txt\",\"content\":\"1\"}"}}]})
                    } else {
                        serde_json::json!({"content":"Done."})
                    };
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":reply,"finish_reason":"stop"}]}))
                })
                .mount(&server)
                .await;
        let llm = crate::providers::resolve(
            &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "synthetic")
                .with_model("test-model")
                .with_base_url(server.uri()),
        )
        .await
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let tools =
            crate::tools::boxed_tools(root.path(), Arc::new(crate::tools::AllowAllHook)).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel(256);
        let policy = RunPolicy {
            total_turns: 8,
            context_token_budget: 24000,
            recent_messages: 4,
        };
        let answer = EVAL_NUDGES_OFF
            .scope(
                // The turn-summary nudge has its own test.
                Arc::new([nudges_off, vec!["turn_summary".to_owned()]].concat()),
                run_task_with_evidence(
                    llm,
                    tools,
                    "Write a.txt with 1.".into(),
                    vec![],
                    Arc::new(crate::tools::AllowAllHook),
                    policy,
                    tx,
                    Arc::new(AtomicBool::new(false)),
                    String::new(),
                    None,
                ),
            )
            .await
            .unwrap();
        let bodies = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|request| request.body_json().unwrap())
            .collect();
        (answer, bodies)
    }
    let checks = |bodies: &[serde_json::Value]| {
        bodies
            .iter()
            .filter(|body| {
                body["messages"].as_array().unwrap().last().unwrap()["content"] == SPEC_CHECK
            })
            .count()
    };
    let (answer, bodies) = run(Vec::new(), true).await;
    // The checked answer continues the first one; the chat shows both, once.
    assert_eq!(answer, "Done.\n\nVerified: a.txt holds 1.");
    assert_eq!((bodies.len(), checks(&bodies)), (3, 1));
    // The answer before the check stays in the conversation the model sees.
    let checked = bodies[2]["messages"].as_array().unwrap();
    assert_eq!(checked[checked.len() - 2]["content"], "Done.");
    let (answer, bodies) = run(Vec::new(), false).await;
    assert_eq!(
        (answer.as_str(), bodies.len(), checks(&bodies)),
        ("Done.", 1, 0)
    );
    let (answer, bodies) = run(vec!["spec_check".into()], true).await;
    assert_eq!(
        (answer.as_str(), bodies.len(), checks(&bodies)),
        ("Done.", 2, 0)
    );
}

/// A run that used tools and ends on a short progress line is asked once to continue or
/// sum up; a summary, a run without tools, or the eval switch leaves it alone.
#[cfg(feature = "eval")]
#[tokio::test]
async fn a_progress_line_ending_is_asked_once_for_the_turns_summary() {
    use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};
    async fn run(
        nudges_off: Vec<String>,
        tools: bool,
        ending: &'static str,
    ) -> (String, Vec<serde_json::Value>) {
        let server = MockServer::builder().start().await;
        Mock::given(method("POST"))
                .respond_with(move |request: &Request| {
                    let body: serde_json::Value = request.body_json().unwrap();
                    let messages = body["messages"].as_array().unwrap();
                    let last = messages.last().unwrap();
                    let reply = if last["content"] == TURN_SUMMARY {
                        serde_json::json!({"content":"I listed the project: it is empty, so there is nothing to change. Next: add the files you want."})
                    } else if tools && !messages.iter().any(|message| message["role"] == "tool") {
                        serde_json::json!({"content":"Looking.","tool_calls":[{"id":"l","type":"function","function":{"name":"list_dir","arguments":"{\"path\":\".\"}"}}]})
                    } else {
                        serde_json::json!({"content":ending})
                    };
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":reply,"finish_reason":"stop"}]}))
                })
                .mount(&server)
                .await;
        let llm = crate::providers::resolve(
            &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "synthetic")
                .with_model("test-model")
                .with_base_url(server.uri()),
        )
        .await
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let tools =
            crate::tools::boxed_tools(root.path(), Arc::new(crate::tools::AllowAllHook)).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel(256);
        let policy = RunPolicy {
            total_turns: 8,
            context_token_budget: 24000,
            recent_messages: 4,
        };
        let answer = EVAL_NUDGES_OFF
            .scope(
                Arc::new(nudges_off),
                run_task_with_evidence(
                    llm,
                    tools,
                    "Look at the project.".into(),
                    vec![],
                    Arc::new(crate::tools::AllowAllHook),
                    policy,
                    tx,
                    Arc::new(AtomicBool::new(false)),
                    String::new(),
                    None,
                ),
            )
            .await
            .unwrap();
        let bodies = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|request| request.body_json().unwrap())
            .collect();
        (answer, bodies)
    }
    let asked = |bodies: &[serde_json::Value]| {
        bodies
            .iter()
            .filter(|body| {
                body["messages"].as_array().unwrap().last().unwrap()["content"] == TURN_SUMMARY
            })
            .count()
    };
    let (answer, bodies) = run(Vec::new(), true, "Continuing.").await;
    // The progress line is not part of the answer; the chat keeps it as a progress line.
    assert!(answer.starts_with("I listed the project"), "{answer}");
    assert_eq!((bodies.len(), asked(&bodies)), (3, 1));
    // The progress line stays in what the model sees.
    let summed = bodies[2]["messages"].as_array().unwrap();
    assert_eq!(summed[summed.len() - 2]["content"], "Continuing.");
    let summary = "I listed the project and it is empty, so I changed nothing.\nNext: tell me which files to add.";
    let (answer, bodies) = run(Vec::new(), true, summary).await;
    assert_eq!(
        (answer.as_str(), bodies.len(), asked(&bodies)),
        (summary, 2, 0)
    );
    let (answer, bodies) = run(Vec::new(), false, "4").await;
    assert_eq!((answer.as_str(), bodies.len(), asked(&bodies)), ("4", 1, 0));
    let (answer, bodies) = run(vec!["turn_summary".into()], true, "Continuing.").await;
    assert_eq!(
        (answer.as_str(), bodies.len(), asked(&bodies)),
        ("Continuing.", 2, 0)
    );
    assert_eq!(joined("Done.", "Done."), "Done.");
    assert_eq!(joined("Done.", "Done. Verified."), "Done. Verified.");
    assert_eq!(joined("Done.", "Verified."), "Done.\n\nVerified.");
    assert!(
        progress_line(" Moving on to the next batch. ")
            && !progress_line("Done.\nAll checks pass.")
    );
    // A short finished answer is a summary; a short line that says the work goes on is not.
    for line in [
        "Continuing through the ember files.",
        "Silver-sorrel corrected — moving on to silver-zephyr.",
        "That vole’s fixed — pulling the next memo.",
        "I’ll check the rest now.",
    ] {
        assert!(progress_line(line), "{line}");
    }
    for line in [
        "Done.",
        "Verified: a.txt holds 1.",
        "Done — count.txt now contains 60, verified with a PASS check.",
        "4",
    ] {
        assert!(!progress_line(line), "{line}");
    }
}

/// A blocking question ends the run after its tool batch, without another model request.
#[tokio::test]
async fn an_end_request_finishes_the_run_after_its_tool_batch() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let server = MockServer::builder().start().await;
    let end: EndRequest = Arc::default();
    let tool_end = Arc::clone(&end);
    Mock::given(method("POST"))
            .respond_with(move |_: &wiremock::Request| {
                // As a blocking question would, mid-batch.
                *tool_end.lock().unwrap() = Some("Waiting for your answer.".into());
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":"Asking.","tool_calls":[{"id":"l","type":"function","function":{"name":"list_dir","arguments":"{\"path\":\".\"}"}}]},"finish_reason":"tool_calls"}]}))
            })
            .mount(&server)
            .await;
    let llm = crate::providers::resolve(
        &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "synthetic")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let tools =
        crate::tools::boxed_tools(root.path(), Arc::new(crate::tools::AllowAllHook)).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let policy = RunPolicy {
        total_turns: 8,
        context_token_budget: 24000,
        recent_messages: 4,
    };
    let answer = with_end_request(
        Arc::clone(&end),
        run_task_with_evidence(
            llm,
            tools,
            "Do the work.".into(),
            vec![],
            Arc::new(crate::tools::AllowAllHook),
            policy,
            tx,
            Arc::new(AtomicBool::new(false)),
            String::new(),
            None,
        ),
    )
    .await
    .unwrap();
    assert_eq!(answer, "Waiting for your answer.");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(end.lock().unwrap().is_none(), "the request is used once");
    let mut finished = None;
    while let Ok(event) = rx.try_recv() {
        if let RunEvent::Finished { result } = event {
            finished = Some(result);
        }
    }
    assert_eq!(finished.as_deref(), Some("Waiting for your answer."));
}

#[tokio::test]
async fn steered_messages_reach_the_next_model_request() {
    use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};
    async fn run(
        first_calls_tool: bool,
        steers: &'static [&'static str],
    ) -> (String, Vec<serde_json::Value>, Vec<RunEvent>) {
        let server = MockServer::builder().start().await;
        let inbox: SteerInbox = Arc::default();
        let pushed = Arc::new(AtomicBool::new(false));
        let (server_inbox, server_pushed) = (Arc::clone(&inbox), Arc::clone(&pushed));
        Mock::given(method("POST"))
                .respond_with(move |request: &Request| {
                    let body: serde_json::Value = request.body_json().unwrap();
                    let steered = body["messages"].as_array().unwrap().iter().any(|message| {
                        message["content"].as_str().is_some_and(|text| text.ends_with("Use tabs."))
                    });
                    // The user steers while the first request is in flight.
                    if !server_pushed.swap(true, Ordering::SeqCst) {
                        let mut queue = server_inbox.lock().unwrap();
                        for text in steers {
                            queue.push_back((*text).into());
                        }
                    }
                    let reply = if steered {
                        serde_json::json!({"content":ADJUSTED})
                    } else if first_calls_tool {
                        serde_json::json!({"content":"Looking.","tool_calls":[{"id":"l","type":"function","function":{"name":"list_dir","arguments":"{\"path\":\".\"}"}}]})
                    } else {
                        serde_json::json!({"content":"Done early."})
                    };
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":reply,"finish_reason":"stop"}]}))
                })
                .mount(&server)
                .await;
        let llm = crate::providers::resolve(
            &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "synthetic")
                .with_model("test-model")
                .with_base_url(server.uri()),
        )
        .await
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let tools =
            crate::tools::boxed_tools(root.path(), Arc::new(crate::tools::AllowAllHook)).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        let policy = RunPolicy {
            total_turns: 8,
            context_token_budget: 24000,
            recent_messages: 4,
        };
        let answer = with_steer_inbox(
            Arc::clone(&inbox),
            run_task_with_evidence(
                llm,
                tools,
                "Format the file.".into(),
                vec![],
                Arc::new(crate::tools::AllowAllHook),
                policy,
                tx,
                Arc::new(AtomicBool::new(false)),
                String::new(),
                None,
            ),
        )
        .await
        .unwrap();
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert!(inbox.lock().unwrap().is_empty());
        let bodies = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|request| request.body_json().unwrap())
            .collect();
        (answer, bodies, events)
    }
    // One steer: no check before the run ends.
    let (answer, bodies, _) = run(true, &["Use tabs."]).await;
    assert_eq!(answer, ADJUSTED);
    assert_eq!(bodies.len(), 2);
    for first_calls_tool in [true, false] {
        let (answer, bodies, events) =
            run(first_calls_tool, &["Use tabs.", "And keep it short."]).await;
        // The same answer before and after the check is shown once.
        assert_eq!(answer, ADJUSTED);
        // Two steers: once, before the run ends, the model is shown both again.
        assert_eq!(bodies.len(), 3);
        let last = bodies[2]["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(last["role"], "user");
        assert_eq!(
            last["content"],
            steers_check(&["Use tabs.".into(), "And keep it short.".into()])
        );
        assert!(last["content"]
            .as_str()
            .unwrap()
            .contains("\n1. Use tabs.\n2. And keep it short.\n"));
        let messages = bodies[1]["messages"].as_array().unwrap();
        let steered: Vec<_> = messages
            .iter()
            .filter(|message| message["role"] == "user")
            .filter_map(|message| message["content"].as_str())
            .filter(|text| text.starts_with("[The user sent this while you were working"))
            .collect();
        assert_eq!(
            steered,
            [
                steered_message("Use tabs."),
                steered_message("And keep it short.")
            ]
        );
        let seen: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                RunEvent::Steered { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(seen, ["Use tabs.", "And keep it short."]);
        if !first_calls_tool {
            // The early answer stays in the conversation the model sees.
            assert!(messages.iter().any(
                |message| message["role"] == "assistant" && message["content"] == "Done early."
            ));
        }
    }
}

#[cfg(feature = "eval")]
#[tokio::test]
async fn eval_switches_turn_off_only_the_named_nudges() {
    assert!(!nudge_off("loop_warning"));
    EVAL_NUDGES_OFF
        .scope(Arc::new(vec!["loop_warning".to_owned()]), async {
            assert!(nudge_off("loop_warning"));
            assert!(!nudge_off("spec_check"));
            assert!(!nudge_off("repeat_receipt"));
        })
        .await;
}

#[test]
fn rate_limits_and_cut_off_answers_wait_longer_within_the_answer_deadline() {
    use autoagents::llm::error::LLMError;
    assert_eq!(answer_retry_limit(true, false), 5);
    assert_eq!(answer_retry_limit(false, true), 4);
    assert_eq!(answer_retry_limit(false, false), 2);
    let limited = LLMError::RateLimitError {
        status_code: 429,
        message: "Rate limit exceeded".into(),
        response_body: "".into(),
        retry_after: None,
        provider_code: None,
    };
    let waits: Vec<u64> = (0..RATE_LIMIT_RETRIES)
        .map(|retry| {
            request_retry_delay_with_rate_limit_floor(&limited, retry, RATE_LIMIT_BACKOFF_SECONDS)
                .as_secs()
        })
        .collect();
    for (retry, wait) in waits.iter().enumerate() {
        let base = RATE_LIMIT_BACKOFF_SECONDS << retry;
        assert!((base..=base * 3 / 2).contains(wait), "{waits:?}");
    }
    // The longest jittered waits (310 s base, at most 465 s) plus five more 60-second
    // attempts still fit the 900-second answer deadline.
    assert!(waits.iter().sum::<u64>() <= 465, "{waits:?}");
}

#[tokio::test]
async fn answer_deadline_and_stop_cancel_server_backoff_and_permanent_errors_do_not_retry() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    for (status, cancel) in [
        (503, false),
        (499, false),
        (503, true),
        (401, false),
        (400, false),
    ] {
        let server = MockServer::builder().start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).insert_header("Retry-After", "45"))
            .mount(&server)
            .await;
        let llm = crate::providers::resolve(
            &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "test-key")
                .with_model("test-model")
                .with_base_url(server.uri()),
        )
        .await
        .unwrap();
        let stopped = AtomicBool::new(false);
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let request = request_answer_with_timeout(
            &llm,
            &[],
            &[],
            &tx,
            &stopped,
            std::time::Duration::from_millis(if cancel { 5000 } else { 150 }),
        );
        let stop = async {
            if cancel {
                while server.received_requests().await.unwrap().is_empty() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                stopped.store(true, Ordering::SeqCst);
            }
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            tokio::join!(request, stop)
        })
        .await
        .unwrap();
        let error = result.unwrap_err();
        if matches!(status, 503 | 499) && !cancel {
            assert!(error.to_string().contains("deadline exceeded"));
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        let mut stopped_events = 0;
        while let Ok(event) = rx.try_recv() {
            stopped_events += usize::from(
                matches!(event, RunEvent::Failed { error } if error.starts_with("Stopped by you")),
            );
        }
        assert_eq!(stopped_events, usize::from(cancel));
    }
}

#[tokio::test]
async fn summary_recovers_on_third_attempt_without_exceeding_retry_output_reserve() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    for failure in ["transient", "empty", "truncated", "malformed", "timeout"] {
        let server = MockServer::builder().start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        Mock::given(method("POST")).respond_with(move |_: &wiremock::Request| {
                let attempt = observed.fetch_add(1, Ordering::SeqCst);
                if failure == "transient" && attempt < 2 {
                    return ResponseTemplate::new(503);
                }
                let text = if attempt >= 2 {
                    "Goal/constraints: preserve. Confirmed findings: source supplied. Completed changes/tests: none. Unresolved work: continue. Next action: answer."
                } else if failure == "empty" { " " } else { "Incomplete notes" };
                let response = ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":text},"finish_reason":if failure == "truncated" && attempt < 2 {"length"} else {"stop"}}]}));
                if failure == "timeout" && attempt < 2 {
                    response.set_delay(std::time::Duration::from_millis(1500))
                } else { response }
            }).mount(&server).await;
        let llm = crate::providers::resolve(
            &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "test-key")
                .with_model("test-model")
                .with_base_url(server.uri()),
        )
        .await
        .unwrap();
        let config = crate::configuration::CompactionConfig {
            request_timeout_seconds: 1,
            ..Default::default()
        };
        let summary = with_configuration(
            config,
            compaction_chat(
                &llm,
                &context_update_messages("Original state", "", "Continue", true),
                512,
            ),
        )
        .await
        .unwrap_or_else(|error| panic!("{failure}: {error}"));
        assert!(summary.contains("Next action: answer"));
        assert_eq!(calls.load(Ordering::SeqCst), 3, "{failure}");
        for (attempt, request) in server.received_requests().await.unwrap().iter().enumerate() {
            let body: serde_json::Value = request.body_json().unwrap();
            assert_eq!(
                body["max_tokens"],
                if attempt == 0 { 512 } else { 1024 },
                "{failure}"
            );
        }
    }
}

#[tokio::test]
async fn summarization_retries_transient_errors_within_limit() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    for limit in [1654, 8192] {
        for (status, recovers, attempts) in [
            (429, true, 2),
            (503, true, 2),
            (503, false, 3),
            (499, true, 2),
            (499, false, 3),
            (401, true, 1),
            (400, true, 1),
        ] {
            let server = MockServer::builder().start().await;
            let calls = Arc::new(AtomicUsize::new(0));
            let observed = Arc::clone(&calls);
            Mock::given(method("POST")).respond_with(move |request: &wiremock::Request| {
                if observed.fetch_add(1, Ordering::SeqCst) == 0 || !recovers {
                    ResponseTemplate::new(status)
                } else {
                    // A transient first attempt must not spend the only retry on the smaller allowance.
                    let body: serde_json::Value = request.body_json().unwrap();
                    let fits = body["max_tokens"] == limit * 2;
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"role":"assistant","content":"Goal/constraints: retain task. Confirmed findings: originals supplied. Completed changes/tests: none. Unresolved work: continue. Next action: Recovered task state"},"finish_reason":if fits {"stop"} else {"length"}}]}))
                }
            }).mount(&server).await;
            let llm = crate::providers::resolve(
                &crate::providers::ProviderConfig::new(
                    crate::providers::ProviderKind::Go,
                    "test-key",
                )
                .with_model("test-model")
                .with_base_url(server.uri()),
            )
            .await
            .unwrap();
            let result = compaction_chat(
                &llm,
                &context_update_messages("Original state", "", "Continue", true),
                limit,
            )
            .await;
            if recovers && attempts == 2 {
                assert_eq!(result.unwrap(), "Goal/constraints: retain task. Confirmed findings: originals supplied. Completed changes/tests: none. Unresolved work: continue. Next action: Recovered task state");
            } else {
                assert!(result.is_err());
            }
            assert_eq!(
                calls.load(Ordering::SeqCst),
                attempts,
                "HTTP {status}, recovers={recovers}"
            );
            for (index, request) in server.received_requests().await.unwrap().iter().enumerate() {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                assert_eq!(body["max_tokens"], limit * if index == 0 { 1 } else { 2 });
            }
        }
    }
}

#[tokio::test]
async fn compaction_deadline_and_stop_cancel_retry_backoff_without_replacing_context() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    for cancel in [false, true] {
        let server = MockServer::builder().start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).insert_header("Retry-After", "45"))
            .mount(&server)
            .await;
        let llm = crate::providers::resolve(
            &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "test-key")
                .with_model("test-model")
                .with_base_url(server.uri()),
        )
        .await
        .unwrap();
        let mut messages = initial_messages(
            vec![ConversationTurn {
                role: ConversationRole::User,
                text: "Original state".repeat(100),
            }],
            "Continue".into(),
        );
        let before = serde_json::to_value(&messages).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let stopped = AtomicBool::new(false);
        let compact = compact_context_with_timeout(
            &llm,
            &mut messages,
            &RunPolicy {
                total_turns: 5,
                context_token_budget: 200_000,
                recent_messages: 4,
            },
            &tx,
            &stopped,
            std::time::Duration::from_millis(if cancel { 5000 } else { 100 }),
        );
        let stop = async {
            if cancel {
                while server.received_requests().await.unwrap().is_empty() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                stopped.store(true, Ordering::SeqCst);
            }
        };
        let (result, _) = tokio::time::timeout(std::time::Duration::from_millis(500), async {
            tokio::join!(compact, stop)
        })
        .await
        .expect("deadline and Stop must preempt the retry delay");
        assert!(result.unwrap_err().to_string().contains(if cancel {
            "Stopped by you"
        } else {
            "summarization timed out"
        }));
        assert_eq!(serde_json::to_value(&messages).unwrap(), before);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn stalled_compaction_times_out_without_replacing_conversation() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(30)).set_body_json(serde_json::json!({"choices":[{"message":{"role":"assistant","content":"Late checkpoint"},"finish_reason":"stop"}]})))
            .mount(&server).await;
    let llm = crate::providers::resolve(
        &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let mut messages = initial_messages(
        vec![ConversationTurn {
            role: ConversationRole::User,
            text: "Original context ".repeat(500),
        }],
        "Continue".into(),
    );
    let before = serde_json::to_value(&messages).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let result = compact_context_with_timeout(
        &llm,
        &mut messages,
        &RunPolicy {
            total_turns: 5,
            context_token_budget: 16_000,
            recent_messages: 4,
        },
        &tx,
        &AtomicBool::new(false),
        std::time::Duration::from_millis(100),
    )
    .await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("summarization timed out"));
    assert_eq!(serde_json::to_value(&messages).unwrap(), before);
    assert!(matches!(
        rx.try_recv().unwrap(),
        RunEvent::ContextCompacting
    ));
    assert!(rx.try_recv().is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn original_derived_reads_survive_maintenance_until_summary() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let (root, mut messages) = recovered_original_excerpt_fixture().await;
    messages.insert(
        0,
        ChatMessage {
            role: ChatRole::System,
            message_type: MessageType::Text,
            content: "System".into(),
        },
    );
    messages.insert(
        1,
        ChatMessage {
            role: ChatRole::User,
            message_type: MessageType::Text,
            content: "Continue repair".into(),
        },
    );
    let ordinary = ToolCall {
        id: "ordinary".into(),
        call_type: "function".into(),
        function: FunctionCall {
            name: "shell".into(),
            arguments: "{}".into(),
        },
    };
    messages.push(ChatMessage {
        role: ChatRole::Assistant,
        message_type: MessageType::ToolUse(vec![ordinary.clone()]),
        content: String::new(),
    });
    let mut output = ordinary;
    output.function.arguments =
        serde_json::json!({"stdout":"old observation ".repeat(1000)}).to_string();
    messages.push(ChatMessage {
        role: ChatRole::Tool,
        message_type: MessageType::ToolResult(vec![output]),
        content: String::new(),
    });
    for index in 0..4 {
        messages.push(ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::Text,
            content: format!("Recent action {index}"),
        });
    }
    let policy = RunPolicy {
        context_token_budget: 24_000,
        recent_messages: 4,
        total_turns: 10,
    };
    messages[2].content = "Public intent ".repeat(500);
    let mut evidence = root.path().join(".themis/context/test");
    for index in 0..6 {
        evidence.push(format!("{index}{}", "p".repeat(99)));
    }
    std::fs::create_dir_all(&evidence).unwrap();
    let before = estimated_tokens(&messages);
    assert!(
        maintenance::maintain(&mut messages, &policy, Some(&evidence), 0)
            .await
            .unwrap()
    );
    assert!(
        estimated_tokens(&messages) < before,
        "old narration and redundant recovery still reduce without a model call"
    );
    let recursive = messages
        .iter()
        .filter_map(|m| match &m.message_type {
            MessageType::ToolResult(items) => items.iter().find(|r| r.id == "recursive"),
            _ => None,
        })
        .next()
        .unwrap();
    assert!(
        recursive.function.arguments.contains("maintenance_receipt"),
        "recursive recovery copies remain reducible"
    );
    let saved = std::fs::read_dir(&evidence).unwrap().count();
    assert!(
        !maintenance::maintain(&mut messages, &policy, Some(&evidence), 0)
            .await
            .unwrap()
    );
    assert_eq!(
        std::fs::read_dir(&evidence).unwrap().count(),
        saved,
        "protected-only maintenance must not save duplicate snapshots"
    );
    assert!(ledger_text(&messages, 0, None).is_empty());
    let (request, _, _) = prepare_compaction(&messages, "Continue repair", 24_000, 1024).unwrap();
    let server = MockServer::builder().start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":"Goal/constraints: preserve. Confirmed findings: check failed. Completed changes/tests: failed test. Unresolved work: fix. Next action: repair."},"finish_reason":"stop"}]}))).mount(&server).await;
    let llm = crate::providers::resolve(
        &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "synthetic")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    compaction_chat(&llm, &request, 1024).await.unwrap();
    let captured = server.received_requests().await.unwrap();
    let body: serde_json::Value = captured[0].body_json().unwrap();
    let input = body["messages"][1]["content"].as_str().unwrap();
    assert_eq!(
        input.matches("RETAIN_PENDING_ASSERTION").count(),
        1,
        "maintenance must not erase acquired original-derived facts before summary preparation"
    );
    assert!(input.contains("source_sha256"));
}

#[tokio::test]
async fn original_derived_unsummarized_tail_survives_the_next_checkpoint() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let (root, observations) = recovered_original_excerpt_fixture().await;
    let mut messages = vec![
        ChatMessage {
            role: ChatRole::System,
            message_type: MessageType::Text,
            content: "System".into(),
        },
        ChatMessage {
            role: ChatRole::User,
            message_type: MessageType::Text,
            content: "Continue repair".into(),
        },
        ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::Text,
            content: "Earlier work".into(),
        },
    ];
    messages.extend(observations);
    let policy = RunPolicy {
        context_token_budget: 24_000,
        recent_messages: 4,
        total_turns: 10,
    };
    let (_, snapshot) =
        archive_context(&root.path().join(".themis/context/test"), &messages).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    replace_context(&mut messages, &policy, 3, "Goal/constraints: preserve. Confirmed findings: earlier work. Completed changes/tests: none. Unresolved work: repair. Next action: inspect.".into(), &tx, Some(&snapshot)).await.unwrap();
    for index in 0..4 {
        messages.push(ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::Text,
            content: format!("Recent work {index}"),
        });
    }
    let split = context_split(&messages, &policy);
    let (request, _, _) =
        prepare_compaction(&messages[..split], "Continue repair", 24_000, 1024).unwrap();
    let server = MockServer::builder().start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":"Goal/constraints: preserve. Confirmed findings: check failed. Completed changes/tests: failed test. Unresolved work: fix. Next action: repair."},"finish_reason":"stop"}]}))).mount(&server).await;
    let llm = crate::providers::resolve(
        &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "synthetic")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    compaction_chat(&llm, &request, 1024).await.unwrap();
    let captured = server.received_requests().await.unwrap();
    let body: serde_json::Value = captured[0].body_json().unwrap();
    let input = body["messages"][1]["content"].as_str().unwrap();
    assert_eq!(input.matches("RETAIN_PENDING_ASSERTION").count(), 1, "the first checkpoint must not strip unsummarized tail read selectors needed by the next checkpoint");
    assert!(input.contains("source_sha256"));
    assert!(evidence_records(&messages)
        .unwrap()
        .iter()
        .any(|r| r.contains("\"evidence_kind\":\"recovery\"")));
}

#[tokio::test]
async fn ordinary_unsummarized_tail_reaches_the_next_checkpoint() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let root = tempfile::tempdir().unwrap();
    let mut messages = initial_messages(Vec::new(), "Analyze the shipping record.".into());
    messages.push(ChatMessage {
        role: ChatRole::Assistant,
        content: "Earlier completed work".into(),
        message_type: MessageType::Text,
    });
    let call=ToolCall {id:"selected".into(),call_type:"function".into(),function:FunctionCall {name:"shell".into(),arguments:serde_json::json!({"command":"python3","args":["-c",format!("# {}\nprint('selected passage')","padding ".repeat(400))]}).to_string()}};
    let mut result = call.clone();
    result.function.arguments=serde_json::json!({"exit_code":0,"output":format!("{}\nThe parcel arrived on Tuesday, not Monday.","context ".repeat(350))}).to_string();
    messages.push(ChatMessage {
        role: ChatRole::Assistant,
        content: String::new(),
        message_type: MessageType::ToolUse(vec![call]),
    });
    messages.push(ChatMessage {
        role: ChatRole::Tool,
        content: String::new(),
        message_type: MessageType::ToolResult(vec![result]),
    });
    let policy = RunPolicy {
        total_turns: 20,
        context_token_budget: 24000,
        recent_messages: 2,
    };
    let evidence = root.path().join(".themis/context/test");
    std::fs::create_dir_all(&evidence).unwrap();
    let (_, snapshot) = archive_context(&evidence, &messages).unwrap();
    let original = std::fs::read(&snapshot).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    replace_context(&mut messages,&policy,3,"Goal/constraints: analyze shipping. Confirmed findings: earlier work. Completed changes/tests: none. Unresolved work: parcel timing. Next action: inspect.".into(),&tx,Some(&snapshot)).await.unwrap();
    let MessageType::ToolUse(calls) = &messages[messages.len() - 2].message_type else {
        panic!("retained call missing")
    };
    let receipt: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
    assert_eq!(
        receipt["arguments_offloaded"], true,
        "large completed scripts must remain offloadable without hiding their result"
    );
    for index in 0..4 {
        messages.push(ChatMessage {
            role: ChatRole::Assistant,
            content: format!("Later work {index}"),
            message_type: MessageType::Text,
        });
    }
    let split = context_split(&messages, &policy);
    let (request, _, _) =
        prepare_compaction(&messages[..split], "Analyze shipping", 24000, 1024).unwrap();
    let server = MockServer::builder().start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":"Goal/constraints: analyze. Confirmed findings: parcel arrived Tuesday. Completed changes/tests: none. Unresolved work: report. Next action: answer."},"finish_reason":"stop"}]}))).mount(&server).await;
    let llm = crate::providers::resolve(
        &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "synthetic")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    compaction_chat(&llm, &request, 1024).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value = requests[0].body_json().unwrap();
    assert!(
        body["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("The parcel arrived on Tuesday, not Monday."),
        "retained ordinary evidence must not become omitted recovery at the next checkpoint"
    );
    assert_eq!(std::fs::read(snapshot).unwrap(), original);
}

#[tokio::test]
async fn fitting_summary_receives_original_derived_failures_without_optional_caches() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let (_root, messages) = recovered_original_excerpt_fixture().await;
    let before = serde_json::to_value(&messages).unwrap();
    assert!(ledger_text(&messages, 0, None).is_empty());
    let (request, _, sampled) =
        prepare_compaction(&messages, "Continue repair", 24_000, 1024).unwrap();
    assert!(!sampled);
    let server = MockServer::builder().start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":"Goal/constraints: preserve. Confirmed findings: check failed. Completed changes/tests: failed test. Unresolved work: fix. Next action: repair."},"finish_reason":"stop"}]}))).mount(&server).await;
    let llm = crate::providers::resolve(
        &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "synthetic")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    compaction_chat(&llm, &request, 1024).await.unwrap();
    let captured = server.received_requests().await.unwrap();
    let body: serde_json::Value = captured[0].body_json().unwrap();
    let input = body["messages"][1]["content"].as_str().unwrap();
    assert_eq!(
        input.matches("RETAIN_PENDING_ASSERTION").count(),
        1,
        "exact original-derived failure must reach the actual summary request once"
    );
    assert!(input.contains("original_derived_excerpt"));
    assert!(input.contains("source_sha256"));
    assert!(input.contains("snapshot-original.jsonl"));
    assert!(!input.contains("RECURSIVE_COPY_MUST_BE_EXCLUDED"));
    assert!(!input.contains("NAVIGATION_COPY_MUST_BE_EXCLUDED"));
    assert_eq!(
        serde_json::to_value(&messages).unwrap(),
        before,
        "summary projection must not reclassify or modify stored recovery observations"
    );
}

#[tokio::test]
async fn oversized_history_compacts_in_one_request_and_leaves_the_rest_to_originals() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let server = MockServer::builder().start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":"Goal/constraints: keep going. Unresolved work: continue. Parked work: none. Next action: continue."},"finish_reason":"stop"}]}))).mount(&server).await;
    let llm = crate::providers::resolve(
        &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "synthetic")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    // Long fields need excerpts; many records need the oldest left to the originals.
    for (records, field) in [(6, 40_000), (400, 200)] {
        let history = (0..records)
            .map(|index| ConversationTurn {
                role: ConversationRole::User,
                text: format!("RECORD-{index} {}", "padding ".repeat(field / 8)),
            })
            .collect();
        let mut messages = initial_messages(history, "Continue".into());
        let (tx, _rx) = tokio::sync::mpsc::channel(64);
        let before = server.received_requests().await.unwrap().len();
        assert!(compact_context_with_evidence(
            &llm,
            &mut messages,
            &RunPolicy {
                total_turns: 8,
                context_token_budget: 6_000,
                recent_messages: 2,
            },
            &tx,
            &AtomicBool::new(false),
            std::time::Duration::from_secs(10),
            Some(directory.path().into()),
        )
        .await
        .unwrap());
        let captured = server.received_requests().await.unwrap();
        assert_eq!(
            captured.len(),
            before + 1,
            "one summary request per checkpoint"
        );
        let body: serde_json::Value = captured.last().unwrap().body_json().unwrap();
        let input = body["messages"][1]["content"].as_str().unwrap();
        assert!(input.contains("coverage is incomplete"), "{records}");
        assert!(
            estimated_tokens(&[ChatMessage {
                role: ChatRole::User,
                message_type: MessageType::Text,
                content: input.to_owned(),
            }]) < 6_000
        );
        if records == 400 {
            assert!(input.contains("oldest new records omitted (only in saved originals)"));
            // History turns are records 1..=400 and the active request 401.
            assert!(
                input.contains("\"source_record\":401"),
                "the newest records are kept"
            );
            assert!(
                !input.contains("\"source_record\":1,"),
                "the oldest are left out"
            );
        }
        let checkpoint = messages
            .iter()
            .find(|m| m.content.starts_with("Earlier context checkpoint"))
            .unwrap();
        assert!(checkpoint.content.contains("Latest snapshot: "));
    }
}

#[tokio::test]
async fn archived_recovery_results_retain_distinct_provenance() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let directory = tempfile::tempdir().unwrap();
    let mut messages = Vec::new();
    for (id, path) in [
        ("original", "story.txt"),
        ("recovery", ".themis/context/chat/snapshot-1.jsonl"),
    ] {
        let call = ToolCall {
            id: id.into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: serde_json::json!({"file_path":path}).to_string(),
            },
        };
        messages.push(ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::ToolUse(vec![call.clone()]),
            content: String::new(),
        });
        let mut result = call;
        result.function.arguments = serde_json::json!({"content":"same passage"}).to_string();
        messages.push(ChatMessage {
            role: ChatRole::Tool,
            message_type: MessageType::ToolResult(vec![result]),
            content: String::new(),
        });
    }
    let MessageType::ToolUse(original_calls) = &messages[0].message_type else {
        panic!("tool use")
    };
    let MessageType::ToolUse(recovery_calls) = &messages[2].message_type else {
        panic!("tool use")
    };
    let MessageType::ToolResult(original_results) = &messages[1].message_type else {
        panic!("tool result")
    };
    let MessageType::ToolResult(recovery_results) = &messages[3].message_type else {
        panic!("tool result")
    };
    messages.extend([
        ChatMessage {
            role: ChatRole::Assistant,
            content: String::new(),
            message_type: MessageType::ToolUse(
                [original_calls.clone(), recovery_calls.clone()].concat(),
            ),
        },
        ChatMessage {
            role: ChatRole::Tool,
            content: String::new(),
            message_type: MessageType::ToolResult(
                [original_results.clone(), recovery_results.clone()].concat(),
            ),
        },
    ]);
    let (_, path) = archive_context(directory.path(), &messages).unwrap();
    let stored = std::fs::read_to_string(path).unwrap();
    let records: Vec<serde_json::Value> = stored
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records[1]["evidence_kind"], "original");
    assert_eq!(records[3]["evidence_kind"], "recovery");
    assert_eq!(records[5]["evidence_kind"], "mixed");
    assert_eq!(
        records[5]["message_type"]["ToolResult"][0]["evidence_kind"],
        "original"
    );
    assert_eq!(
        records[5]["message_type"]["ToolResult"][1]["evidence_kind"],
        "recovery"
    );
    let restored: Vec<ChatMessage> = stored
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        serde_json::to_value(restored).unwrap(),
        serde_json::to_value(&messages).unwrap(),
        "provenance must not alter original observations"
    );
    let provider = MockServer::builder().start().await;
    Mock::given(method("POST")).respond_with(|request: &wiremock::Request| {
            let body: serde_json::Value = request.body_json().unwrap();
            let input = body["messages"][1]["content"].as_str().unwrap();
            let provenance = !input.contains("\"evidence_kind\":\"recovery\"") && input.contains("\"evidence_kind\":\"original\"") && input.contains("duplicate/recovery records omitted");
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"role":"assistant","content":if provenance {"Goal/constraints: retain task. Confirmed findings: originals supplied. Completed changes/tests: none. Unresolved work: continue. Next action: Originals distinguished from recovered copies"} else {"Provenance missing"}},"finish_reason":"stop"}]}))
        }).mount(&provider).await;
    let llm = crate::providers::resolve(
        &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(provider.uri()),
    )
    .await
    .unwrap();
    let mut context = initial_messages(Vec::new(), "Original question".into());
    context.extend(messages);
    context.push(ChatMessage {
        role: ChatRole::User,
        message_type: MessageType::Text,
        content: "Continue".into(),
    });
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    compact_context_with_timeout(
        &llm,
        &mut context,
        &RunPolicy {
            total_turns: 100,
            context_token_budget: 50_000,
            recent_messages: 2,
        },
        &tx,
        &AtomicBool::new(false),
        std::time::Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(
            context[1]
                .content
                .contains("Goal/constraints: retain task. Confirmed findings: originals supplied. Completed changes/tests: none. Unresolved work: continue. Next action: Originals distinguished from recovered copies"),
            "fitting updates must carry the same original/recovery distinction as section requests"
        );
}

#[tokio::test]
async fn fitting_compaction_updates_full_prior_task_state_once() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    for (budget, padding) in [(24_000, 0), (200_000, 0), (200_000, 6_000)] {
        let expected = format!(
            "EARLY_LIMIT_A remains required. {} EARLY_LIMIT_B remains required.",
            "old é🙂 context ".repeat(padding)
        );
        let previous = format!("Earlier context checkpoint (historical; later user requests take precedence):\n{expected}");
        let server = MockServer::builder().start().await;
        Mock::given(method("POST")).respond_with(move |request: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let input = body["messages"][1]["content"].as_str().unwrap();
                let content = if input.contains("NEW MESSAGE RECORDS WITH PROVENANCE") && input.contains(&expected) && input.contains("NEW_OBSERVATION_42") { "EARLY_LIMIT_A and EARLY_LIMIT_B retained with new observations." } else { "New observations only; earlier facts omitted by this section summary." };
                let content = format!("Goal/constraints: retain task. Confirmed findings: originals supplied. Completed changes/tests: none. Unresolved work: continue. Next action: {content}");
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"role":"assistant","content":content},"finish_reason":"stop"}]}))
            }).mount(&server).await;
        let llm = crate::providers::resolve(
            &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "test-key")
                .with_model("test-model")
                .with_base_url(server.uri()),
        )
        .await
        .unwrap();
        let mut messages = initial_messages(
            vec![ConversationTurn {
                role: ConversationRole::Assistant,
                text: previous,
            }],
            "Continue the same task".into(),
        );
        messages.insert(
            2,
            ChatMessage {
                role: ChatRole::User,
                message_type: MessageType::Text,
                content: "NEW_OBSERVATION_42".into(),
            },
        );
        for _ in 0..4 {
            messages.insert(
                messages.len() - 1,
                ChatMessage {
                    role: ChatRole::Assistant,
                    message_type: MessageType::Text,
                    content: "Recent result".into(),
                },
            );
        }
        let call = ToolCall {
            id: "latest-test".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "shell".into(),
                arguments: r#"{"command":"python3","args":["-m","unittest"]}"#.into(),
            },
        };
        messages.push(ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::ToolUse(vec![call.clone()]),
            content: "Changes saved; run the tests before edge checks.".into(),
        });
        let mut result = call;
        result.function.arguments = r#"{"exit_code":0,"output":"LATEST_TEST_16_OK"}"#.into();
        messages.push(ChatMessage {
            role: ChatRole::Tool,
            message_type: MessageType::ToolResult(vec![result]),
            content: String::new(),
        });
        let (tx, _) = tokio::sync::mpsc::channel(16);
        compact_context_with_timeout(
            &llm,
            &mut messages,
            &RunPolicy {
                total_turns: 200,
                context_token_budget: budget,
                recent_messages: 4,
            },
            &tx,
            &AtomicBool::new(false),
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert!(
            messages[1]
                .content
                .contains("EARLY_LIMIT_A and EARLY_LIMIT_B retained"),
            "the full prior task state must bypass lossy section notes"
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests.len(),
            1,
            "fitting context needs one state update, not section summaries plus a merge"
        );
        let body: serde_json::Value = requests[0].body_json().unwrap();
        let input = body["messages"][1]["content"].as_str().unwrap();
        assert_eq!(
            input.matches("EARLY_LIMIT_A").count(),
            1,
            "previous checkpoint must be supplied only once"
        );
        assert!(input.contains("NEW_OBSERVATION_42"));
        assert!(
            input.contains("LATEST_TEST_16_OK"),
            "the summary must reconcile the newest retained observations, not produce a stale plan"
        );
        assert!(matches!(
            messages.last().unwrap().message_type,
            MessageType::ToolResult(_)
        ));
        assert_eq!(
            body["max_tokens"],
            if budget == 24_000 { 3000 } else { 8192 }
        );
    }
}

#[tokio::test]
async fn blocked_checkpoint_event_does_not_replace_context_before_timeout() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let server = MockServer::builder().start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"role":"assistant","content":"Short summary"},"finish_reason":"stop"}]}))).mount(&server).await;
    let llm = crate::providers::resolve(
        &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let mut messages = initial_messages(
        vec![ConversationTurn {
            role: ConversationRole::User,
            text: "Original goal and evidence".into(),
        }],
        "Continue".into(),
    );
    let before = serde_json::to_value(&messages).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let result = compact_context_with_timeout(
        &llm,
        &mut messages,
        &RunPolicy {
            total_turns: 200,
            context_token_budget: 200_000,
            recent_messages: 4,
        },
        &tx,
        &AtomicBool::new(false),
        std::time::Duration::from_millis(100),
    )
    .await;
    assert!(result.is_err());
    assert!(
        !result
            .unwrap_err()
            .to_string()
            .contains("/retry-compaction"),
        "a job without an evidence directory is not durably resumable"
    );
    assert_eq!(
        serde_json::to_value(&messages).unwrap(),
        before,
        "event backpressure must not leave a replaced context after a failed checkpoint"
    );
    assert!(matches!(
        rx.try_recv().unwrap(),
        RunEvent::ContextCompacting
    ));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn storage_failure_preserves_context_without_manual_retry() {
    use wiremock::MockServer;
    // A read-only directory does not refuse new files on Windows.
    let failures = ["archive"]
        .into_iter()
        .chain((!cfg!(windows)).then_some("unwritable"));
    for failure in failures {
        let server = MockServer::builder().start().await;
        let llm = crate::providers::resolve(
            &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "test-key")
                .with_model("test-model")
                .with_base_url(server.uri()),
        )
        .await
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let evidence = directory.path().join("evidence");
        let mut messages = initial_messages(
            vec![ConversationTurn {
                role: ConversationRole::User,
                text: "Original goal and evidence".repeat(500),
            }],
            "Continue".into(),
        );
        let before = serde_json::to_value(&messages).unwrap();
        if failure == "archive" {
            std::fs::write(&evidence, "cannot hold evidence").unwrap();
        } else {
            // A read-only evidence directory refuses the snapshot itself.
            std::fs::create_dir(&evidence).unwrap();
            let mut permissions = std::fs::metadata(&evidence).unwrap().permissions();
            permissions.set_readonly(true);
            std::fs::set_permissions(&evidence, permissions).unwrap();
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let error = compact_context_with_evidence(
            &llm,
            &mut messages,
            &RunPolicy {
                total_turns: 8,
                context_token_budget: 24_000,
                recent_messages: 2,
            },
            &tx,
            &AtomicBool::new(false),
            std::time::Duration::from_secs(5),
            Some(evidence.clone()),
        )
        .await
        .unwrap_err()
        .to_string();
        assert_eq!(serde_json::to_value(&messages).unwrap(), before);
        assert!(server.received_requests().await.unwrap().is_empty());
        assert!(rx.try_recv().is_err());
        assert!(
            error.contains("working context unchanged"),
            "{failure}: {error}"
        );
        assert!(!error.contains("/retry-compaction"), "{failure}: {error}");
        assert!(
            error.contains("Originals were not saved on disk"),
            "{failure}: {error}"
        );
        if failure == "unwritable" {
            let mut permissions = std::fs::metadata(&evidence).unwrap().permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            std::fs::set_permissions(&evidence, permissions).unwrap();
            assert_eq!(std::fs::read_dir(&evidence).unwrap().count(), 0);
        }
    }
}
