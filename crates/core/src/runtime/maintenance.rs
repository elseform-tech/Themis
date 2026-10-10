//! Cheap working-context maintenance. Immutable snapshots keep exact observations.
use super::*;

pub(super) async fn maintain(
    messages: &mut Vec<ChatMessage>,
    policy: &RunPolicy,
    directory: Option<&std::path::Path>,
) -> anyhow::Result<bool> {
    let Some(directory) = directory else {
        return Ok(false);
    };
    let split = context_split(messages, policy);
    let candidates: Vec<usize> = (1..split)
        .filter(|&i| {
            matches!(
                messages[i].message_type,
                MessageType::ToolUse(_) | MessageType::ToolResult(_)
            ) && serde_json::to_vec(&messages[i]).is_ok_and(|v| v.len() > 2048)
        })
        .collect();
    if candidates.is_empty() {
        return Ok(false);
    }
    let before = estimated_tokens(messages);
    let (_, path) = save_evidence(directory, messages).await?;
    let mut next = messages.clone();
    for index in candidates {
        let (kind, calls) = match &mut next[index].message_type {
            MessageType::ToolUse(calls) => ("ToolUse", calls),
            MessageType::ToolResult(calls) => ("ToolResult", calls),
            _ => continue,
        };
        for (call_index, call) in calls.iter_mut().enumerate() {
            if call
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
                "interaction_completed":true, "invocation_is_not_execution_proof":kind=="ToolUse",
                "outcome":outcome, "preview":utf8_prefix(&call.function.arguments,256),
                "original":{"path":path,"jsonl_record":index,"json_pointer":format!("/message_type/{kind}/{call_index}/function/arguments")},
                "instruction":"Reopen the immutable original for missing facts; do not execute the tool again to recover output."}).to_string();
        }
        if next[index].content.len() > 1024 {
            next[index].content = format!(
                "{}\nPublic intent excerpt only; original record {index} /content in {}",
                utf8_prefix(&next[index].content, 768),
                path.display()
            );
        }
    }
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
    #[tokio::test]
    async fn maintenance_preserves_exact_original_without_reexecution_and_is_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let call = ToolCall {
            id: "done-1".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "shell".into(),
                arguments: serde_json::json!({"exit_code":0,"stdout":"evidence 中 ".repeat(10000)})
                    .to_string(),
            },
        };
        let mut messages = initial_messages(Vec::new(), "current task".into());
        messages.insert(
            1,
            ChatMessage {
                role: ChatRole::Tool,
                content: String::new(),
                message_type: MessageType::ToolResult(vec![call.clone()]),
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
        assert!(maintain(&mut messages, &policy, Some(directory.path()))
            .await
            .unwrap());
        let MessageType::ToolResult(calls) = &messages[1].message_type else {
            panic!("lost observation");
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
        assert_eq!(receipt["outcome"]["exit_code"], 0);
        assert!(!maintain(&mut messages, &policy, Some(directory.path()))
            .await
            .unwrap());
    }
}
