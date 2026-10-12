//! Cheap working-context maintenance. Immutable snapshots keep exact observations.
use super::*;

/// `summarized_until` is the first message index a checkpoint has not yet
/// summarized. Earlier results already reached the summarizer and may become
/// receipts; later acquired observations stay protected until they do.
pub(super) async fn maintain(
    messages: &mut Vec<ChatMessage>,
    policy: &RunPolicy,
    directory: Option<&std::path::Path>,
    summarized_until: usize,
) -> anyhow::Result<bool> {
    let Some(directory) = directory else {
        return Ok(false);
    };
    let split = context_split(messages, policy);
    let mut protected = summary_evidence::protected_entries(messages);
    protected.retain(|(index, _)| *index >= summarized_until);
    let candidates: Vec<usize> = (1..split)
        .filter(|&i| {
            let calls = match &messages[i].message_type {
                MessageType::ToolUse(calls) | MessageType::ToolResult(calls) => calls,
                _ => return false,
            };
            let protected_result = matches!(messages[i].message_type, MessageType::ToolResult(_))
                && calls
                    .iter()
                    .enumerate()
                    .any(|(j, _)| protected.contains(&(i, j)));
            serde_json::to_vec(&messages[i]).is_ok_and(|v| v.len() > 2048)
                && ((messages[i].content.len() > 1024 && !protected_result)
                    || calls.iter().enumerate().any(|(j, call)| {
                        !protected.contains(&(i, j))
                            && !call
                                .function
                                .arguments
                                .contains("\"maintenance_receipt\":true")
                    }))
        })
        .collect();
    if candidates.is_empty() {
        return Ok(false);
    }
    let before = estimated_tokens(messages);
    // A conservative archive name lets us reject no-op/expanding receipts
    // without publishing a complete snapshot on every subsequent request.
    let planned_path = directory.join(format!("snapshot-{}.jsonl", u128::MAX));
    let planned = reduced_context(messages, &candidates, &protected, &planned_path);
    if estimated_tokens(&planned) >= before {
        return Ok(false);
    }
    let (_, path) = save_evidence(directory, messages).await?;
    let next = reduced_context(messages, &candidates, &protected, &path);
    let after = estimated_tokens(&next);
    if after >= before {
        return Ok(false);
    }
    *messages = next;
    crate::diagnostics::emit(
        "context",
        "maintenance_completed",
        "info",
        serde_json::json!({
            "estimated_before_tokens":before,"estimated_after_tokens":after,"saved_tokens":before-after
        }),
    );
    Ok(true)
}

fn reduced_context(
    messages: &[ChatMessage],
    candidates: &[usize],
    protected: &HashSet<(usize, usize)>,
    path: &std::path::Path,
) -> Vec<ChatMessage> {
    let mut next = messages.to_vec();
    for &index in candidates {
        let (kind, calls) = match &mut next[index].message_type {
            MessageType::ToolUse(calls) => ("ToolUse", calls),
            MessageType::ToolResult(calls) => ("ToolResult", calls),
            _ => continue,
        };
        let protected_result = kind == "ToolResult"
            && calls
                .iter()
                .enumerate()
                .any(|(j, _)| protected.contains(&(index, j)));
        for (call_index, call) in calls.iter_mut().enumerate() {
            if protected.contains(&(index, call_index))
                || call
                    .function
                    .arguments
                    .contains("\"maintenance_receipt\":true")
            {
                continue;
            }
            let observed: serde_json::Value =
                serde_json::from_str(&call.function.arguments).unwrap_or_default();
            let outcome = serde_json::json!({"success":observed.get("success"),"status":observed.get("status"),"exit_code":observed.get("exit_code"),"is_error":observed.get("is_error")});
            call.function.arguments = serde_json::json!({"maintenance_receipt":true,
                "original_context_recovery":summary_evidence::recovery_call(call),
                "interaction_completed":true, "invocation_is_not_execution_proof":kind=="ToolUse",
                "outcome":outcome, "preview":utf8_prefix(&call.function.arguments,256),
                "original":{"path":path,"jsonl_record":index,"json_pointer":format!("/message_type/{kind}/{call_index}/function/arguments")},
                "instruction":summary_evidence::MAINTENANCE_NOTE}).to_string();
        }
        if next[index].content.len() > 1024 && !protected_result {
            let location_note = |file: &str| {
                format!("\nPublic intent excerpt only; original record {index} /content in saved evidence file {file}")
            };
            let note = location_note(&path.file_name().unwrap_or_default().to_string_lossy());
            // Use identical excerpt space for planning and publication. Shorter
            // actual names must not expose more JSON-escaped narration bytes.
            let maximum_note = location_note(&format!("snapshot-{}.jsonl", u128::MAX));
            next[index].content = format!(
                "{}{note}",
                utf8_prefix(
                    &next[index].content,
                    1024usize.saturating_sub(maximum_note.len().max(note.len()))
                )
            );
        }
    }
    next
}

fn utf8_prefix(text: &str, limit: usize) -> &str {
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maintenance_planning_bounds_escaped_paths_and_narration() {
        let mut messages = initial_messages(Vec::new(), "Continue".into());
        messages.push(ChatMessage {
            role: ChatRole::Assistant,
            content: "\"".repeat(1200),
            message_type: MessageType::ToolUse(vec![ToolCall {
                id: "protected-source".into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: "read_file".into(),
                    arguments: "x".repeat(3000),
                },
            }]),
        });
        let protected = HashSet::from([(2, 0)]);
        let short_name = "snapshot-1.jsonl";
        let note =
            format!("\nPublic intent excerpt only; original record 2 /content in /{short_name}");
        let directory = std::path::PathBuf::from("\"".repeat(1000 - note.len()));
        let planned = reduced_context(
            &messages,
            &[2],
            &protected,
            &directory.join(format!("snapshot-{}.jsonl", u128::MAX)),
        );
        let actual = reduced_context(&messages, &[2], &protected, &directory.join(short_name));
        assert!(
            estimated_tokens(&planned) >= estimated_tokens(&actual),
            "maximum filename planning must conservatively bound actual JSON escaping"
        );
        assert!(actual[2].content.len() <= 1024);
    }

    #[tokio::test]
    async fn maintenance_keeps_plain_and_shell_facts_in_the_next_provider_request() {
        use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
        let root = tempfile::tempdir().unwrap();
        let source = format!(
            "{}\nThe sealed parcel belongs to Mira, not Tomas.\n",
            "padding ".repeat(350)
        );
        std::fs::write(root.path().join("story.txt"), &source).unwrap();
        let tools =
            crate::tools::boxed_tools(root.path(), Arc::new(crate::tools::AllowAllHook)).unwrap();
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":"Observed."},"finish_reason":"stop"}]}))).mount(&server).await;
        let llm = crate::providers::resolve(
            &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "synthetic")
                .with_model("test-model")
                .with_base_url(server.uri()),
        )
        .await
        .unwrap();
        for (name, args) in [
            ("read_file", serde_json::json!({"file_path":"story.txt"})),
            (
                "shell",
                if cfg!(windows) {
                    serde_json::json!({"command":"cmd","args":["/C","type","story.txt"]})
                } else {
                    serde_json::json!({"command":"cat","args":["story.txt"]})
                },
            ),
        ] {
            let tool = tools.iter().find(|t| t.name() == name).unwrap();
            let output = tool.execute(args.clone()).await.unwrap();
            let call = ToolCall {
                id: name.into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: name.into(),
                    arguments: args.to_string(),
                },
            };
            let mut result = call.clone();
            result.function.arguments = output.to_string();
            assert!(result.function.arguments.find("The sealed parcel").unwrap() > 256);
            let mut messages = initial_messages(Vec::new(), "Answer who owns the parcel.".into());
            messages.push(ChatMessage {
                role: ChatRole::Assistant,
                content: "Old public intent ".repeat(250),
                message_type: MessageType::ToolUse(vec![call]),
            });
            messages.push(ChatMessage {
                role: ChatRole::Tool,
                content: String::new(),
                message_type: MessageType::ToolResult(vec![result]),
            });
            for index in 0..6 {
                messages.push(ChatMessage {
                    role: ChatRole::Assistant,
                    content: format!("Other work {index}"),
                    message_type: MessageType::Text,
                });
            }
            let evidence = root.path().join(".themis/context/test");
            std::fs::create_dir_all(&evidence).unwrap();
            let policy = RunPolicy {
                total_turns: 20,
                context_token_budget: 24000,
                recent_messages: 2,
            };
            assert!(maintain(&mut messages, &policy, Some(&evidence), 0)
                .await
                .unwrap());
            llm.chat_with_tools(&messages, None, None).await.unwrap();
            let requests = server.received_requests().await.unwrap();
            let body: serde_json::Value = requests.last().unwrap().body_json().unwrap();
            assert!(
                body["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["role"] == "tool"
                        && m["content"].as_str().is_some_and(
                            |s| s.contains("The sealed parcel belongs to Mira, not Tomas.")
                        )),
                "maintenance erased substantive {name} evidence before semantic preservation"
            );
            assert_eq!(
                std::fs::read_to_string(root.path().join("story.txt")).unwrap(),
                source
            );
        }
    }

    #[tokio::test]
    async fn maintenance_noop_does_not_publish_snapshots_repeatedly() {
        let directory = tempfile::tempdir().unwrap();
        let calls: Vec<ToolCall> = [
            ("acquired", "story.txt"),
            ("recovery", ".themis/context/chat/snapshot.jsonl"),
        ]
        .into_iter()
        .map(|(id, path)| ToolCall {
            id: id.into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: serde_json::json!({"file_path":path}).to_string(),
            },
        })
        .collect();
        let mut results = calls.clone();
        results[0].function.arguments =
            serde_json::json!({"content":"acquired fact ".repeat(300)}).to_string();
        results[1].function.arguments = "{}".into();
        let mut messages = initial_messages(Vec::new(), "Answer from acquired facts".into());
        messages.push(ChatMessage {
            role: ChatRole::Assistant,
            content: String::new(),
            message_type: MessageType::ToolUse(calls),
        });
        messages.push(ChatMessage {
            role: ChatRole::Tool,
            content: String::new(),
            message_type: MessageType::ToolResult(results),
        });
        for _ in 0..6 {
            messages.push(ChatMessage {
                role: ChatRole::Assistant,
                content: "Later work".into(),
                message_type: MessageType::Text,
            });
        }
        let original = serde_json::to_value(&messages).unwrap();
        let policy = RunPolicy {
            total_turns: 20,
            context_token_budget: 24000,
            recent_messages: 2,
        };
        for _ in 0..2 {
            assert!(!maintain(&mut messages, &policy, Some(directory.path()), 0)
                .await
                .unwrap());
            assert_eq!(serde_json::to_value(&messages).unwrap(), original);
            assert_eq!(
                std::fs::read_dir(directory.path()).unwrap().count(),
                0,
                "a known no-op must not archive the entire context on every turn"
            );
        }
    }

    #[tokio::test]
    async fn summarized_results_become_reducible_but_new_observations_stay_protected() {
        let directory = tempfile::tempdir().unwrap();
        let call = ToolCall {
            id: "acquired".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: serde_json::json!({"file_path":"story.txt"}).to_string(),
            },
        };
        let mut result = call.clone();
        result.function.arguments =
            serde_json::json!({"content":"acquired fact ".repeat(300)}).to_string();
        let mut messages = initial_messages(Vec::new(), "Answer from acquired facts".into());
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
        let summarized = messages.len();
        for _ in 0..6 {
            messages.push(ChatMessage {
                role: ChatRole::Assistant,
                content: "Later work".into(),
                message_type: MessageType::Text,
            });
        }
        let policy = RunPolicy {
            total_turns: 20,
            context_token_budget: 24000,
            recent_messages: 2,
        };
        assert!(
            !maintain(&mut messages, &policy, Some(directory.path()), 0)
                .await
                .unwrap(),
            "an observation the summarizer has not seen stays protected"
        );
        assert!(
            maintain(&mut messages, &policy, Some(directory.path()), summarized)
                .await
                .unwrap(),
            "after a checkpoint summarized it, the result can become a receipt"
        );
        let MessageType::ToolResult(results) = &messages[3].message_type else {
            panic!("result missing")
        };
        let receipt: serde_json::Value =
            serde_json::from_str(&results[0].function.arguments).unwrap();
        assert_eq!(receipt["maintenance_receipt"], true);
        let path = receipt["original"]["path"].as_str().unwrap();
        assert!(std::fs::read_to_string(path)
            .unwrap()
            .contains("acquired fact"));
    }

    #[tokio::test]
    async fn maintenance_preserves_exact_original_without_reexecution_and_is_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let call = ToolCall {
            id: "done-1".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "write_file".into(),
                arguments: serde_json::json!({"file_path":"generated.py","content":"evidence 中 ".repeat(10000)})
                    .to_string(),
            },
        };
        let mut messages = initial_messages(Vec::new(), "current task".into());
        messages.insert(
            1,
            ChatMessage {
                role: ChatRole::Assistant,
                content: String::new(),
                message_type: MessageType::ToolUse(vec![call.clone()]),
            },
        );
        let mut result = call.clone();
        result.function.arguments =
            serde_json::json!({"success":true,"bytes_written":100000}).to_string();
        messages.insert(
            2,
            ChatMessage {
                role: ChatRole::Tool,
                content: String::new(),
                message_type: MessageType::ToolResult(vec![result.clone()]),
            },
        );
        for _ in 0..8 {
            messages.push(ChatMessage {
                role: ChatRole::User,
                content: "latest request".into(),
                message_type: MessageType::Text,
            });
        }
        let policy = RunPolicy {
            total_turns: 20,
            context_token_budget: 24000,
            recent_messages: 4,
        };
        assert!(maintain(&mut messages, &policy, Some(directory.path()), 0)
            .await
            .unwrap());
        let MessageType::ToolUse(calls) = &messages[1].message_type else {
            panic!("lost completed invocation");
        };
        let receipt: serde_json::Value =
            serde_json::from_str(&calls[0].function.arguments).unwrap();
        let record =
            std::fs::read_to_string(receipt["original"]["path"].as_str().unwrap()).unwrap();
        let original: serde_json::Value =
            serde_json::from_str(record.lines().next().unwrap()).unwrap();
        assert_eq!(
            original
                .pointer(receipt["original"]["json_pointer"].as_str().unwrap())
                .unwrap(),
            &call.function.arguments
        );
        assert_eq!(receipt["invocation_is_not_execution_proof"], true);
        let MessageType::ToolResult(results) = &messages[2].message_type else {
            panic!("lost result");
        };
        assert_eq!(results[0].function.arguments, result.function.arguments);
        assert!(!maintain(&mut messages, &policy, Some(directory.path()), 0)
            .await
            .unwrap());
    }
}
