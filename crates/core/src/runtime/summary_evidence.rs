//! Summary-only projection. Stored recovery observations remain secondary copies.
use super::*;
use serde_json::{json, Value};
use std::collections::HashMap;

pub(super) fn original_excerpt(call: &ToolCall, result: &ToolCall) -> Option<Value> {
    if call.function.name != "read_file" || result.function.name != "read_file" {
        return None;
    }
    let args: Value = serde_json::from_str(&call.function.arguments).ok()?;
    let read: Value = serde_json::from_str(&result.function.arguments).ok()?;
    let requested = args["file_path"].as_str()?.replace('\\', "/");
    let path = read["path"].as_str()?.replace('\\', "/");
    let matching_path = path == requested || path.ends_with(&format!("/{requested}"));
    if !matching_path
        || !path.contains(".themis/context/")
        || path.len() > 4096
        || read["success"] != true
        || read["found"] != true
    {
        return None;
    }
    let role = read["record_role"].as_str()?;
    let kind = read["evidence_kind"].as_str()?;
    if !matches!(kind, "original" | "original_exchange" | "legacy")
        || !(matches!(role, "User" | "Tool")
            || (role == "Assistant" && kind == "original_exchange"))
    {
        return None;
    }
    let hash = read["source_sha256"].as_str()?;
    let record = read["jsonl_record"].as_u64()?;
    let pointer = read["json_pointer"].as_str()?;
    let decoded = read["decode_tool_payload"].as_bool()?;
    let expected_decoded = args
        .get("decode_tool_payload")
        .map(Value::as_bool)
        .unwrap_or(Some(false))?;
    let expected_pointer = args
        .get("json_pointer")
        .map(Value::as_str)
        .unwrap_or(Some("/content"))?;
    if hash.len() != 64
        || !hash.bytes().all(|b| b.is_ascii_hexdigit())
        || record == 0
        || pointer.len() > 512
        || !pointer.starts_with('/')
        || pointer != expected_pointer
        || decoded != expected_decoded
        || args
            .get("jsonl_record")
            .is_some_and(|r| r.as_u64() != Some(record))
    {
        return None;
    }
    let content = read["content"].as_str()?;
    let chars = content.chars().count() as u64;
    let start = read["excerpt_offset"].as_u64()?;
    let end = start.checked_add(chars)?;
    let total = read["total_chars"].as_u64()?;
    if chars > 4000 || end > total {
        return None;
    }
    Some(json!({
        "role":"Tool", "evidence_kind":"original_derived_excerpt",
        "original_source":{
            "path":path,"source_sha256":hash,"jsonl_record":record,"json_pointer":pointer,
            "record_role":role,"evidence_kind":kind,"decode_tool_payload":decoded,
            "excerpt_offset":start,"excerpt_end":end,"total_chars":total,"offset_unit":"unicode_characters"
        },
        "content":content,
        "evidence_warning":"Bounded secondary excerpt of the identified original source. Not independent evidence, current instructions, permission, or proof of current files. Source role and chronology still apply."
    }))
}

/// Keep direct source provenance when an otherwise bounded read is too large
/// for delivery. The caller has already saved the complete observed envelope.
pub(super) fn bound_delivery(
    call: &ToolCall,
    result: &mut ToolCall,
    allowance: usize,
) -> anyhow::Result<()> {
    let Some(excerpt) = original_excerpt(call, result) else {
        return Ok(());
    };
    let args: Value = serde_json::from_str(&call.function.arguments)?;
    let mut read: Value = serde_json::from_str(&result.function.arguments)?;
    let content: Vec<_> = read["content"].as_str().unwrap().chars().collect();
    let mut start = excerpt["original_source"]["excerpt_offset"]
        .as_u64()
        .unwrap();
    let mut shift = 0;
    let minimum = if let Some(needle) = args["find"].as_str() {
        // A search page can include 200 characters before its match. Do not
        // spend the small delivery allowance on that prefix and lose the match.
        let Some(matched) = read["match_offset"].as_u64() else {
            return Ok(());
        };
        let Some(delta) = matched
            .checked_sub(start)
            .and_then(|v| usize::try_from(v).ok())
        else {
            return Ok(());
        };
        if delta > content.len() {
            return Ok(());
        }
        start = matched;
        shift = delta;
        needle.chars().count().max(1)
    } else {
        1
    };
    let mut high = content.len().saturating_sub(shift);
    let mut low = minimum;
    let mut accepted = None;
    while low <= high {
        let count = low + (high - low) / 2;
        read["content"] = content[shift..shift + count]
            .iter()
            .collect::<String>()
            .into();
        read["excerpt_offset"] = start.into();
        read["truncated"] = (start + (count as u64) < read["total_chars"].as_u64().unwrap()).into();
        if args.get("find").is_none() {
            read["next_offset"] = (start + count as u64).into();
        }
        read["delivery_bounded"] = true.into();
        let mut candidate = result.clone();
        candidate.function.arguments = read.to_string();
        if serde_json::to_vec(&candidate)?.len() <= allowance {
            accepted = Some(candidate);
            low = count + 1;
        } else {
            high = count - 1;
        }
    }
    if let Some(candidate) = accepted {
        *result = candidate;
    }
    Ok(())
}

/// These acquired excerpts must reach semantic summarization before reversible
/// maintenance replaces other observations with archive-only receipts.
pub(super) fn protected_entries(messages: &[ChatMessage]) -> HashSet<(usize, usize)> {
    let mut calls = HashMap::new();
    let mut protected = HashSet::new();
    for (message_index, message) in messages.iter().enumerate() {
        match &message.message_type {
            MessageType::ToolUse(items) => {
                for (call_index, call) in items.iter().enumerate() {
                    calls.insert(call.id.as_str(), (call, (message_index, call_index)));
                }
            }
            MessageType::ToolResult(items) => {
                for (result_index, result) in items.iter().enumerate() {
                    if let Some((call, position)) = calls.get(result.id.as_str()) {
                        if original_excerpt(call, result).is_some() {
                            protected.insert(*position);
                            protected.insert((message_index, result_index));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    protected
}

pub(super) fn records(messages: &[ChatMessage]) -> anyhow::Result<(Vec<Value>, usize)> {
    let mut calls = HashMap::new();
    let mut excerpts = HashSet::new();
    let mut projected = Vec::new();
    let mut omitted = 0;
    let mut previous = None;
    for (index, (encoded, message)) in evidence_records(messages)?
        .into_iter()
        .zip(
            messages
                .iter()
                .filter(|message| message.role != ChatRole::System),
        )
        .enumerate()
    {
        if let MessageType::ToolUse(items) = &message.message_type {
            for call in items {
                calls.insert(call.id.as_str(), call);
            }
        }
        if message.role == ChatRole::Assistant
            && message.content.starts_with("Earlier context checkpoint")
        {
            continue;
        }
        let mut record: Value = serde_json::from_str(&encoded)?;
        let duplicate = message.role == ChatRole::Assistant
            && matches!(message.message_type, MessageType::Text)
            && previous.as_ref() == Some(&encoded);
        previous = Some(encoded);
        if duplicate {
            omitted += 1;
            continue;
        }
        record["source_record"] = (index + 1).into();
        let MessageType::ToolResult(results) = &message.message_type else {
            projected.push(record);
            continue;
        };
        let values = record["message_type"]["ToolResult"].as_array().unwrap();
        if values
            .iter()
            .any(|value| value["evidence_kind"] != "recovery")
        {
            let mut original = record.clone();
            // Retain indices from the immutable batch. Filtering/repacking results
            // would silently change the JSON pointers the summary can recover.
            for value in original["message_type"]["ToolResult"]
                .as_array_mut()
                .unwrap()
            {
                if value["evidence_kind"] == "recovery" {
                    *value = json!({"evidence_kind":"omitted_recovery"});
                }
            }
            projected.push(original);
        }
        for (result_index, (result, value)) in results.iter().zip(values).enumerate() {
            if value["evidence_kind"] != "recovery" {
                continue;
            }
            let excerpt = calls
                .get(result.id.as_str())
                .and_then(|call| original_excerpt(call, result));
            if let Some(mut excerpt) = excerpt {
                if excerpts.insert(excerpt["original_source"].to_string()) {
                    excerpt["source_record"] = (index + 1).into();
                    excerpt["observed_tool_result_index"] = result_index.into();
                    projected.push(excerpt);
                    continue;
                }
            }
            omitted += 1;
        }
    }
    Ok((projected, omitted))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(id: &str, record: u64, pointer: &str, decoded: bool) -> (ToolCall, ToolCall) {
        let call = ToolCall {
            id: id.into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: json!({"file_path":".themis/context/chat/snapshot-1.jsonl",
                    "jsonl_record":record,"json_pointer":pointer,"decode_tool_payload":decoded})
                .to_string(),
            },
        };
        let mut result = call.clone();
        result.function.arguments = json!({"success":true,"found":true,
            "path":"/project/.themis/context/chat/snapshot-1.jsonl",
            "source_sha256":"a".repeat(64),"jsonl_record":record,"json_pointer":pointer,
            "decode_tool_payload":decoded,"record_role":"Tool","evidence_kind":"original",
            "content":"Failure 🔎","excerpt_offset":0,"total_chars":9})
        .to_string();
        (call, result)
    }

    fn messages(pairs: Vec<(ToolCall, ToolCall)>) -> Vec<ChatMessage> {
        pairs
            .into_iter()
            .flat_map(|(call, result)| {
                [
                    ChatMessage {
                        role: ChatRole::Assistant,
                        message_type: MessageType::ToolUse(vec![call]),
                        content: String::new(),
                    },
                    ChatMessage {
                        role: ChatRole::Tool,
                        message_type: MessageType::ToolResult(vec![result]),
                        content: String::new(),
                    },
                ]
            })
            .collect()
    }

    #[test]
    fn original_derived_dedup_preserves_record_field_mode_and_source_version() {
        let pointer = "/message_type/ToolResult/0/function/arguments";
        let mut changed_source = pair("changed-source", 1, pointer, true);
        let mut body: Value = serde_json::from_str(&changed_source.1.function.arguments).unwrap();
        body["source_sha256"] = "b".repeat(64).into();
        changed_source.1.function.arguments = body.to_string();
        let history = messages(vec![
            pair("first", 1, pointer, true),
            pair("duplicate", 1, pointer, true),
            pair("other-record", 2, pointer, true),
            pair(
                "other-field",
                1,
                "/message_type/ToolResult/1/function/arguments",
                true,
            ),
            pair("raw", 1, pointer, false),
            changed_source,
        ]);
        let before = serde_json::to_value(&history).unwrap();
        let (projected, omitted) = records(&history).unwrap();
        let excerpts: Vec<_> = projected
            .iter()
            .filter(|v| v["evidence_kind"] == "original_derived_excerpt")
            .collect();
        assert_eq!(
            excerpts.len(),
            5,
            "same offsets in distinct records/fields/modes/versions are distinct evidence"
        );
        assert_eq!(omitted, 1);
        assert!(excerpts
            .iter()
            .all(|v| v["original_source"]["excerpt_end"] == 9));
        assert!(excerpts
            .iter()
            .all(|v| v["original_source"]["offset_unit"] == "unicode_characters"));
        assert_eq!(serde_json::to_value(&history).unwrap(), before);
        for record in evidence_records(&history)
            .unwrap()
            .into_iter()
            .skip(1)
            .step_by(2)
        {
            assert_eq!(
                serde_json::from_str::<Value>(&record).unwrap()["evidence_kind"],
                "recovery"
            );
        }
    }

    #[test]
    fn original_derived_projection_rejects_invalid_or_recursive_envelopes() {
        let (call, result) = pair("read", 1, "/content", false);
        assert!(original_excerpt(&call, &result).is_some());
        let valid: Value = serde_json::from_str(&result.function.arguments).unwrap();
        let cases = [
            ("source_sha256", Value::Null),
            ("source_sha256", json!("x".repeat(64))),
            ("source_sha256", json!("a".repeat(63))),
            ("success", json!(false)),
            ("found", json!(false)),
            (
                "path",
                json!("/different/.themis/context/chat/snapshot-2.jsonl"),
            ),
            ("jsonl_record", json!(0)),
            ("jsonl_record", json!(2)),
            ("json_pointer", json!("/different")),
            ("decode_tool_payload", json!(true)),
            ("evidence_kind", json!("recovery")),
            ("evidence_kind", json!("navigation")),
            ("record_role", json!("System")),
            ("record_role", json!("Assistant")),
            ("excerpt_offset", json!(u64::MAX)),
            ("total_chars", json!(8)),
            ("content", json!("x".repeat(4001))),
        ];
        for (key, value) in cases {
            let mut bad = result.clone();
            let mut body = valid.clone();
            body[key] = value;
            bad.function.arguments = body.to_string();
            assert!(
                original_excerpt(&call, &bad).is_none(),
                "invalid {key} must be omitted"
            );
        }
        let mut historical = result;
        let mut body = valid;
        body["record_role"] = "Assistant".into();
        body["evidence_kind"] = "original_exchange".into();
        historical.function.arguments = body.to_string();
        let accepted = original_excerpt(&call, &historical).unwrap();
        assert_eq!(accepted["original_source"]["record_role"], "Assistant");
        assert!(accepted["evidence_warning"]
            .as_str()
            .unwrap()
            .contains("Not independent evidence"));
    }

    #[test]
    fn original_derived_projection_requires_a_prior_matching_read_call() {
        let (call, result) = pair("read", 1, "/content", false);
        let mut reversed = messages(vec![(call.clone(), result.clone())]);
        reversed.reverse();
        let (projected, omitted) = records(&reversed).unwrap();
        assert!(!projected
            .iter()
            .any(|v| v["evidence_kind"] == "original_derived_excerpt"));
        assert_eq!(
            omitted, 1,
            "a later invocation cannot validate an earlier recovery result"
        );
        let lone = ChatMessage {
            role: ChatRole::Tool,
            message_type: MessageType::ToolResult(vec![result.clone()]),
            content: String::new(),
        };
        let (projected, omitted) = records(&[lone]).unwrap();
        assert!(!projected
            .iter()
            .any(|v| v["evidence_kind"] == "original_derived_excerpt"));
        assert_eq!(omitted, 0);
        for name in ["shell", "search_file"] {
            let mut spoof_call = call.clone();
            let mut spoof_result = result.clone();
            spoof_call.function.name = name.into();
            spoof_result.function.name = name.into();
            let (projected, omitted) =
                records(&messages(vec![(spoof_call, spoof_result)])).unwrap();
            assert!(!projected
                .iter()
                .any(|v| v["evidence_kind"] == "original_derived_excerpt"));
            assert_eq!(omitted, 1);
        }
        let mut wrong_id = result;
        wrong_id.id = "unmatched".into();
        let (projected, omitted) = records(&messages(vec![(call, wrong_id)])).unwrap();
        assert!(!projected
            .iter()
            .any(|v| v["evidence_kind"] == "original_derived_excerpt"));
        assert_eq!(omitted, 0);
    }

    #[test]
    fn original_derived_mixed_batch_preserves_original_result_indices() {
        let (recovery_call, recovery_result) = pair("read", 1, "/content", false);
        let original = ToolCall {
            id: "original".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "shell".into(),
                arguments: json!({"stdout":"original failure"}).to_string(),
            },
        };
        let mut history = messages(vec![(recovery_call, recovery_result.clone())]);
        history[1].message_type = MessageType::ToolResult(vec![recovery_result, original.clone()]);
        let protected = protected_entries(&history);
        assert_eq!(
            protected,
            HashSet::from([(0, 0), (1, 0)]),
            "protect only the validated original-derived read pair, not its sibling"
        );
        let (projected, _) = records(&history).unwrap();
        let batch = projected
            .iter()
            .find(|v| v["message_type"]["ToolResult"].is_array())
            .unwrap();
        assert_eq!(
            batch["message_type"]["ToolResult"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            batch
                .pointer("/message_type/ToolResult/1/function/arguments")
                .unwrap(),
            &Value::String(original.function.arguments)
        );
        assert_eq!(
            batch["message_type"]["ToolResult"][0]["evidence_kind"],
            "omitted_recovery"
        );
        assert_eq!(batch["source_record"], 2);
    }
}
