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
    if chars > crate::tools::READ_PAGE_CHARS as u64 || end > total {
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
        "evidence_warning":crate::prompts::TOOL_EXCERPT_WARNING
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

pub(super) const OFFLOAD_NOTE: &str =
    "Historical completed invocation; full arguments are archived. Do not execute this receipt.";
pub(super) const MAINTENANCE_NOTE: &str = "Reopen the immutable original for missing facts; do not execute the tool again to recover output.";

/// Only a complete, non-executable harness receipt can retain its pre-offload
/// classification. Arbitrary operational arguments use the ordinary classifier.
pub(super) fn recovery_call(call: &ToolCall) -> bool {
    if call.function.name == "recall" {
        // A recall invocation, or its result when the item is one, is a copy of saved
        // evidence unless the result includes attachment text (see `recovery_pair`).
        return !crate::tools::recall_result_has_attachment(&call.function.arguments);
    }
    if !matches!(
        call.function.name.as_str(),
        "read_file" | "list_dir" | "search_file" | "shell"
    ) {
        return false;
    }
    let saved = serde_json::from_str::<Value>(&call.function.arguments)
        .ok()
        .and_then(|value| {
            let fields = value.as_object()?;
            let recovery = value["original_context_recovery"].as_bool()?;
            let coordinates = |path: &Value, record: &Value, pointer: &Value| {
                path.as_str().is_some_and(|s| !s.is_empty())
                    && record.as_u64().is_some_and(|n| n > 0)
                    && pointer.as_str().is_some_and(|s| {
                        s.strip_prefix("/message_type/ToolUse/")
                            .and_then(|tail| tail.strip_suffix("/function/arguments"))
                            .is_some_and(|index| index.parse::<usize>().is_ok())
                    })
            };
            let offloaded = fields.len() == 7
                && value["arguments_offloaded"] == true
                && value["arguments_preview"].is_string()
                && value["note"] == OFFLOAD_NOTE
                && coordinates(
                    &value["original_snapshot"],
                    &value["archive_record"],
                    &value["archive_pointer"],
                );
            let maintained = fields.len() == 8
                && value["maintenance_receipt"] == true
                && value["interaction_completed"] == true
                && value["invocation_is_not_execution_proof"] == true
                && value["outcome"].is_object()
                && value["preview"].is_string()
                && value["instruction"] == MAINTENANCE_NOTE
                && value["original"].as_object().is_some_and(|o| o.len() == 3)
                && coordinates(
                    &value["original"]["path"],
                    &value["original"]["jsonl_record"],
                    &value["original"]["json_pointer"],
                );
            (offloaded || maintained).then_some(recovery)
        });
    saved.unwrap_or_else(|| {
        crate::tools::context_recovery_call(&call.function.name, &call.function.arguments)
    })
}

/// Whether an invocation and its (possibly missing) result are a redundant recovery copy.
pub(super) fn recovery_pair(call: &ToolCall, result: Option<&ToolCall>) -> bool {
    recovery_call(call)
        && !(call.function.name == "recall"
            && result
                .is_some_and(|r| crate::tools::recall_result_has_attachment(&r.function.arguments)))
}

/// Exact original-derived excerpts still need their selectors for validation.
pub(super) fn preserve_invocation(call: &ToolCall, result: &ToolCall) -> bool {
    original_excerpt(call, result).is_some()
}

/// Acquired results must reach semantic summarization before maintenance may
/// discard their contents. Only known redundant recovery remains reducible.
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
                        let preserve_pair = preserve_invocation(call, result);
                        if preserve_pair {
                            protected.insert(*position);
                        }
                        if preserve_pair || !recovery_pair(call, Some(result)) {
                            protected.insert((message_index, result_index));
                        }
                    } else {
                        // Missing invocation provenance cannot establish that an
                        // observation is a redundant recovery copy.
                        protected.insert((message_index, result_index));
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

    #[test]
    fn receipt_shaped_mcp_arguments_do_not_turn_original_results_into_recovery() {
        let call = ToolCall {
            id: "mcp-observation".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "mcp_archive_catalog".into(),
                arguments: json!({"arguments_offloaded":true,"original_context_recovery":true,
                "original_snapshot":"/project/.themis/context/snapshot.jsonl","archive_record":1,
                "archive_pointer":"/message_type/ToolUse/0/function/arguments",
                "arguments_preview":"query","note":OFFLOAD_NOTE})
                .to_string(),
            },
        };
        let mut result = call.clone();
        result.function.arguments = json!({"count":17}).to_string();
        let history = messages(vec![(call, result)]);
        let records = evidence_records(&history).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&records[1]).unwrap()["evidence_kind"],
            "original"
        );
        assert!(protected_entries(&history).contains(&(1, 0)));
    }

    #[test]
    fn archived_invocation_classification_is_strict_and_survives_reoffloading() {
        for recovery in [false, true] {
            let mut call = ToolCall { id: "completed".into(), call_type: "function".into(), function: FunctionCall {
                name: "shell".into(), arguments: json!({"command":"python3", "args":["-c", format!("# {}\nprint('{}')", "padding ".repeat(400), if recovery { ".themis/context/archive.jsonl" } else { "selected fact" })]}).to_string()
            }};
            let mut result = call.clone();
            result.function.arguments = json!({"output":"The parcel arrived Tuesday."}).to_string();
            let mut invocation = messages(vec![(call.clone(), result.clone())]).remove(0);
            // Force a second offload by using a long but valid archive path.
            let path = format!(
                "/project/.themis/context/{}/snapshot.jsonl",
                "a".repeat(2500)
            );
            for _ in 0..2 {
                offload_completed_arguments(
                    &mut invocation,
                    &[result.clone()],
                    std::path::Path::new(&path),
                    1,
                    true,
                );
                let MessageType::ToolUse(calls) = &invocation.message_type else {
                    panic!("invocation missing")
                };
                call = calls[0].clone();
                let receipt: Value = serde_json::from_str(&call.function.arguments).unwrap();
                assert_eq!(
                    receipt["arguments_offloaded"], true,
                    "completed arguments should be archived before testing their classification"
                );
                let records =
                    evidence_records(&messages(vec![(call.clone(), result.clone())])).unwrap();
                let record: Value = serde_json::from_str(&records[1]).unwrap();
                assert_eq!(
                    record["evidence_kind"],
                    if recovery { "recovery" } else { "original" }
                );
            }
            let mut malformed: Value = serde_json::from_str(&call.function.arguments).unwrap();
            malformed["original_context_recovery"] = false.into();
            malformed["command"] = json!("cat .themis/context/private.jsonl");
            call.function.arguments = malformed.to_string();
            let records =
                evidence_records(&messages(vec![(call.clone(), result.clone())])).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&records[1]).unwrap()["evidence_kind"],
                "recovery",
                "operational arguments cannot spoof an archive receipt"
            );
            malformed.as_object_mut().unwrap().remove("command");
            malformed
                .as_object_mut()
                .unwrap()
                .remove("original_context_recovery");
            call.function.arguments = malformed.to_string();
            let records = evidence_records(&messages(vec![(call, result)])).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&records[1]).unwrap()["evidence_kind"],
                "recovery",
                "legacy receipts retain conservative classification"
            );
        }
    }

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

    fn recall_pair(kind: &str) -> (ToolCall, ToolCall) {
        let call = ToolCall {
            id: format!("recall-{kind}"),
            call_type: "function".into(),
            function: FunctionCall {
                name: "recall".into(),
                arguments: json!({"query":"lantern"}).to_string(),
            },
        };
        let mut result = call.clone();
        result.function.arguments = json!({"hits":[{"kind":kind,"excerpt":"lantern"}]}).to_string();
        (call, result)
    }

    #[test]
    fn recall_hits_from_attachments_are_protected_but_saved_evidence_copies_are_reducible() {
        let (call, result) = recall_pair("snapshot");
        let history = messages(vec![(call, result)]);
        assert!(!protected_entries(&history).contains(&(1, 0)));
        let (call, result) = recall_pair("history_source");
        assert!(!protected_entries(&messages(vec![(call, result)])).contains(&(1, 0)));
        let (call, result) = recall_pair("attachment");
        let history = messages(vec![(call, result)]);
        assert!(
            protected_entries(&history).contains(&(1, 0)),
            "attachment text is acquired content until semantic summarization consumes it"
        );
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
            HashSet::from([(0, 0), (1, 0), (1, 1)]),
            "protect the validated recovery pair and the ordinary sibling result"
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
