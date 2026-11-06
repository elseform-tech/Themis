use super::*;

#[tokio::test]
async fn absolute_paths_inside_the_project_read_like_relative_ones() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("notes.txt"), "the HERON-42 note").unwrap();
    std::fs::write(outside.path().join("secret.txt"), "outside").unwrap();
    let tools = boxed_tools(project.path(), Arc::new(AllowAllHook)).unwrap();
    let read = tools
        .iter()
        .find(|tool| tool.name() == "read_file")
        .unwrap();
    let canonical = project.path().canonicalize().unwrap();
    for path in [
        project.path().join("notes.txt"),
        canonical.join("notes.txt"),
    ] {
        let result = read
            .execute(serde_json::json!({"file_path": path}))
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        assert!(result.to_string().contains("HERON-42"), "{result}");
    }
    // Outside the project, directly or by climbing out, stays refused.
    let escape = canonical.join("..").join(
        outside
            .path()
            .canonicalize()
            .unwrap()
            .strip_prefix(canonical.parent().unwrap())
            .unwrap_or(outside.path())
            .join("secret.txt"),
    );
    for path in [outside.path().join("secret.txt"), escape] {
        assert!(
            read.execute(serde_json::json!({"file_path": path}))
                .await
                .is_err(),
            "{} must be refused",
            path.display()
        );
    }
}
use std::fs;
use tempfile::tempdir;

fn allow_all() -> Arc<dyn ApprovalHook> {
    Arc::new(AllowAllHook)
}

fn deny_all() -> Arc<dyn ApprovalHook> {
    Arc::new(DenyAllHook)
}

fn rooted_case() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir(&root).unwrap();
    (tmp, root)
}

/// Hook that records every consulted action and answers with `decision`.
struct RecordingHook {
    decision: Approval,
    seen: std::sync::Mutex<Vec<ToolAction>>,
}

impl RecordingHook {
    fn new(decision: Approval) -> Self {
        Self {
            decision,
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn seen(&self) -> Vec<ToolAction> {
        self.seen.lock().unwrap().clone()
    }
}

impl ApprovalHook for RecordingHook {
    fn approve(&self, action: &ToolAction) -> Approval {
        self.seen.lock().unwrap().push(action.clone());
        self.decision
    }
}

fn find_tool<'a>(tools: &'a [Box<dyn ToolT>], name: &str) -> &'a dyn ToolT {
    tools
        .iter()
        .find(|tool| tool.name() == name)
        .map(|tool| &**tool)
        .unwrap()
}

#[tokio::test]
async fn gated_write_denied_under_deny_all() {
    let (_tmp, root) = rooted_case();
    let tools = boxed_tools(&root, deny_all()).unwrap();
    let err = find_tool(&tools, "write_file")
        .execute(json!({
            "file_path": "note.txt",
            "content": "hello",
            "append": false,
        }))
        .await
        .unwrap_err();
    let message = err.to_string().to_lowercase();
    assert!(message.contains("approv"), "{message}");
    assert!(!root.join("note.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn gated_write_os_error_names_the_file() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, root) = rooted_case();
    fs::write(root.join("locked.txt"), "old").unwrap();
    fs::set_permissions(root.join("locked.txt"), fs::Permissions::from_mode(0o444)).unwrap();
    let tools = boxed_tools(&root, allow_all()).unwrap();
    let err = find_tool(&tools, "write_file")
        .execute(json!({"file_path": "locked.txt", "content": "new"}))
        .await
        .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("os error"), "{message}");
    assert!(message.contains("locked.txt"), "{message}");
}

#[tokio::test]
async fn gated_write_works_under_allow_all() {
    let (_tmp, root) = rooted_case();
    let tools = boxed_tools(&root, allow_all()).unwrap();
    let result = find_tool(&tools, "write_file")
        .execute(json!({
            "file_path": "sub/note.txt",
            "content": "hello",
            "append": false,
        }))
        .await
        .unwrap();
    assert_eq!(result["success"].as_bool(), Some(true));
    assert_eq!(
        fs::read_to_string(root.join("sub/note.txt")).unwrap(),
        "hello"
    );
}

#[tokio::test]
async fn gated_read_consults_hook_with_read_risk() {
    // Core asserts the wrapper *consulted* the hook with Read risk; the
    // auto-allow-reads policy itself lives in the desktop hook, so here a
    // recording hook answers AllowAlways and the read succeeds.
    let (_tmp, root) = rooted_case();
    fs::write(root.join("r.txt"), "data").unwrap();
    let hook = Arc::new(RecordingHook::new(Approval::AllowAlways));
    let tools = boxed_tools(&root, hook.clone()).unwrap();
    let result = find_tool(&tools, "read_file")
        .execute(json!({ "file_path": "r.txt" }))
        .await
        .unwrap();
    assert_eq!(result["success"].as_bool(), Some(true));
    assert_eq!(result["content"].as_str(), Some("data"));
    let seen = hook.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].tool, "read_file");
    assert_eq!(seen[0].risk, RiskLevel::Read);
    assert!(!seen[0].summary.is_empty());
}

#[tokio::test]
async fn invalid_record_guides_original_recovery() {
    let (_tmp, root) = rooted_case();
    let original = format!(
        "{}\n{}\n",
        json!({"role":"User","content":"Early original 🔎 RETAIN-753"}),
        json!({"role":"Assistant","content":"Navigation only"})
    );
    fs::write(root.join("early.jsonl"), &original).unwrap();
    let tools = boxed_tools(&root, Arc::new(RecordingHook::new(Approval::AllowOnce))).unwrap();
    let tool = find_tool(&tools, "read_file");
    let error = tool
        .execute(json!({"file_path":"early.jsonl","jsonl_record":3,"find":"Early original"}))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("Total records: 2"), "{error}");
    assert!(error.contains(r#"{"record":1,"role":"User"}"#), "{error}");
    assert!(error.contains("omit jsonl_record"), "{error}");
    assert!(
        !error.contains("RETAIN-753"),
        "record metadata must not expose content"
    );
    let recovered = tool
        .execute(json!({"file_path":"early.jsonl","find":"Early original"}))
        .await
        .unwrap();
    assert_eq!(recovered["jsonl_record"], 1);
    assert_eq!(recovered["record_role"], "User");
    assert!(recovered["content"]
        .as_str()
        .unwrap()
        .contains("RETAIN-753"));
    assert_eq!(
        fs::read_to_string(root.join("early.jsonl")).unwrap(),
        original
    );
    let many = (0..10)
        .map(|_| json!({"role":"UNTRUSTED_ROLE_DO_NOT_ECHO","content":"PRIVATE_BODY"}).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(root.join("many.jsonl"), many).unwrap();
    let error = tool
        .execute(json!({"file_path":"many.jsonl","jsonl_record":999}))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Total records: 10") && error.contains(r#"{"record":8,"role":"Unknown"}"#)
    );
    assert!(!error.contains(r#"{"record":9"#));
    assert!(!error.contains("UNTRUSTED_ROLE") && !error.contains("PRIVATE_BODY"));
}

#[tokio::test]
async fn archive_search_skips_recovery_outputs_and_decodes_tool_payloads() {
    let (_tmp, root) = rooted_case();
    let payload = json!({"stdout":"Original 🔎 passage\nquoted text", "exit_code":0}).to_string();
    let records = [
        json!({"role":"Tool","evidence_kind":"recovery","message_type":{"ToolResult":[{"function":{"arguments":json!({"stdout":"Original 🔎 copied search result"}).to_string()}}]}}),
        json!({"role":"Tool","evidence_kind":"original","message_type":{"ToolResult":[{"function":{"arguments":payload}}]}}),
    ];
    fs::write(
        root.join("observations.jsonl"),
        records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let tools = boxed_tools(&root, Arc::new(RecordingHook::new(Approval::AllowOnce))).unwrap();
    let tool = find_tool(&tools, "read_file");
    let result = tool.execute(json!({"file_path":"observations.jsonl","json_pointer":"/message_type/ToolResult/0/function/arguments","find":"Original 🔎","decode_tool_payload":true})).await.unwrap();
    assert_eq!(
        result["jsonl_record"], 2,
        "generated recovery results must not outrank originals"
    );
    assert!(
        result["content"]
            .as_str()
            .unwrap()
            .contains("passage\nquoted text"),
        "search decoded tool strings, not escaped argument JSON"
    );
    assert_eq!(result["evidence_kind"], "original");
    use sha2::Digest;
    let complete = fs::read(root.join("observations.jsonl")).unwrap();
    assert_eq!(
        result["source_sha256"],
        format!("{:x}", sha2::Sha256::digest(&complete))
    );
    let raw = tool.execute(json!({"file_path":"observations.jsonl","jsonl_record":2,"json_pointer":"/message_type/ToolResult/0/function/arguments"})).await.unwrap();
    assert_eq!(
        raw["source_sha256"], result["source_sha256"],
        "raw and decoded excerpts identify the same full source version"
    );
    assert_eq!(
        raw["content"], payload,
        "existing raw receipt reads must remain compatible"
    );
    let recovery = tool.execute(json!({"file_path":"observations.jsonl","jsonl_record":1,"json_pointer":"/message_type/ToolResult/0/function/arguments"})).await.unwrap();
    assert_eq!(recovery["evidence_kind"], "recovery");
    assert!(recovery["evidence_warning"]
        .as_str()
        .unwrap()
        .contains("original"));
    fs::write(root.join("recovery-only.jsonl"), records[0].to_string()).unwrap();
    let absent = tool.execute(json!({"file_path":"recovery-only.jsonl","find":"copied search result","json_pointer":"/message_type/ToolResult/0/function/arguments"})).await.unwrap();
    assert_eq!(
        absent["found"], false,
        "recovery-only archives must not silently become originals"
    );
    let mixed = json!({"role":"Tool","evidence_kind":"mixed","message_type":{"ToolResult":[
        {"evidence_kind":"recovery","function":{"arguments":"copied recovery passage"}},
        {"evidence_kind":"original","function":{"arguments":"independent original passage"}}
    ]}});
    fs::write(root.join("mixed.jsonl"), mixed.to_string()).unwrap();
    let secondary = tool.execute(json!({"file_path":"mixed.jsonl","find":"passage","json_pointer":"/message_type/ToolResult/0/function/arguments"})).await.unwrap();
    assert_eq!(
        secondary["found"], false,
        "skip only the recovery entry in a mixed batch"
    );
    let independent = tool.execute(json!({"file_path":"mixed.jsonl","find":"passage","json_pointer":"/message_type/ToolResult/1/function/arguments"})).await.unwrap();
    assert_eq!(independent["content"], "independent original passage");
    assert_eq!(independent["evidence_kind"], "original");
    let mut updated = records;
    updated[0]["message_type"]["ToolResult"][0]["function"]["arguments"] =
        "changed other record".into();
    fs::write(
        root.join("observations.jsonl"),
        updated
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let changed = tool.execute(json!({"file_path":"observations.jsonl","jsonl_record":2,"json_pointer":"/message_type/ToolResult/0/function/arguments"})).await.unwrap();
    assert_eq!(changed["content"], raw["content"]);
    assert_ne!(
        changed["source_sha256"], raw["source_sha256"],
        "source version covers the full file, including records outside the excerpt"
    );
}

#[test]
fn original_assistant_exchanges_are_searchable_without_becoming_instructions() {
    let records = [
        json!({"role":"User","evidence_kind":"original_exchange","content":"Discuss options"}),
        json!({"role":"Assistant","evidence_kind":"original_exchange","content":"We proposed AMBER-only; no implementation authorized."}),
        json!({"role":"Assistant","evidence_kind":"navigation","content":"Copied AMBER-only summary"}),
    ];
    let result = bounded_read_result(&json!({"find":"AMBER-only"}), json!({"path":"history.jsonl","content":records.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")})).unwrap();
    assert_eq!(result["jsonl_record"], 2);
    assert_eq!(result["evidence_kind"], "original_exchange");
    assert!(!result["evidence_warning"]
        .as_str()
        .unwrap_or("")
        .contains("not original"));
    assert!(result["evidence_warning"]
        .as_str()
        .unwrap()
        .contains("untrusted"));
}

#[test]
fn jsonl_selectors_on_plain_source_explain_how_to_read_it() {
    let source = "import unittest\nclass TestSettlement(unittest.TestCase):\n    pass\n";
    let error = bounded_read_result(
        &json!({"jsonl_record":1,"json_pointer":"/content"}),
        json!({"path":"tests/test_settlement.py","content":source}),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("JSONL"), "{error}");
    assert!(
        error.contains("omit jsonl_record and json_pointer"),
        "{error}"
    );
    assert!(
        !error.contains(source),
        "source content is not an error diagnostic"
    );
    let result = bounded_read_result(
        &json!({}),
        json!({"path":"tests/test_settlement.py","content":source}),
    )
    .unwrap();
    assert_eq!(result["content"], source);
}

#[test]
fn jsonl_selection_preserves_content_based_semantics() {
    let args = json!({"jsonl_record":1,"json_pointer":"/content"});
    let valid = bounded_read_result(&args, json!({"path":"notes.txt","content":json!({"role":"User","content":"Original fact"}).to_string()})).unwrap();
    assert_eq!(valid["content"], "Original fact");
    assert_eq!(valid["jsonl_record"], 1);
    let error = bounded_read_result(
        &args,
        json!({"path":"broken.jsonl","content":"private malformed source"}),
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("Selected JSONL record is malformed"),
        "{error}"
    );
    assert!(!error.contains("private malformed source"));
}

#[test]
fn generic_jsonl_search_still_finds_untyped_records() {
    let result = bounded_read_result(&json!({"find":"needle"}), json!({"path":"events.jsonl","content":json!({"event":"done","content":"needle"}).to_string()})).unwrap();
    assert_eq!(
        result["found"], true,
        "ordinary non-chat JSONL remains searchable"
    );
}

#[tokio::test]
async fn bounded_read_decodes_originals_and_pages_without_shell_or_path_escape() {
    let (_tmp, root) = rooted_case();
    let original = format!(
        "{}Original 🔎 instruction\nsecond line{}",
        "é".repeat(5000),
        "z".repeat(5000)
    );
    let dump = format!(
        "{}\n{}\n",
        json!({"role":"Assistant","content":format!("Navigation only\nSource: snapshot-original.jsonl\n{}Original 🔎 summary navigation", "x".repeat(5000))}),
        json!({"role":"User","content":original})
    );
    fs::write(root.join("evidence.jsonl"), &dump).unwrap();
    let hook = Arc::new(RecordingHook::new(Approval::AllowOnce));
    let tools = boxed_tools(&root, hook.clone()).unwrap();
    let tool = find_tool(&tools, "read_file");
    assert!(tool.args_schema()["properties"]["jsonl_record"].is_object());
    let result = tool
        .execute(json!({"file_path":"evidence.jsonl","jsonl_record":2,"find":"Original 🔎"}))
        .await
        .unwrap();
    assert_eq!(result["record_role"], "User");
    assert_eq!(result["total_records"], 2);
    assert_eq!(result["match_offset"], 5000);
    let automatic = tool
        .execute(json!({"file_path":"evidence.jsonl","find":"Original 🔎"}))
        .await
        .unwrap();
    assert_eq!(automatic["record_role"], "User");
    assert_eq!(automatic["jsonl_record"], 2);
    assert!(automatic.get("evidence_warning").is_none());
    let secondary_only = tool
        .execute(json!({"file_path":"evidence.jsonl","find":"summary navigation"}))
        .await
        .unwrap();
    assert_eq!(secondary_only["found"], false);
    assert_eq!(secondary_only["content"], "");
    assert!(secondary_only["search_scope"]
        .as_str()
        .unwrap()
        .contains("Original User/Tool"));
    let navigation = tool
        .execute(json!({"file_path":"evidence.jsonl","jsonl_record":1,"find":"summary navigation"}))
        .await
        .unwrap();
    assert_eq!(navigation["record_role"], "Assistant");
    assert!(navigation["evidence_warning"]
        .as_str()
        .unwrap()
        .contains("not original evidence"));
    assert_eq!(
        navigation["navigation_header"],
        "Navigation only\nSource: snapshot-original.jsonl"
    );
    assert!(!navigation["content"].as_str().unwrap().contains("Source:"));
    fs::write(
        root.join("notes.jsonl"),
        json!({"role":"Assistant","content":"Navigation only\nsummary navigation"}).to_string(),
    )
    .unwrap();
    let notes = tool
        .execute(json!({"file_path":"notes.jsonl","find":"summary navigation"}))
        .await
        .unwrap();
    assert_eq!(notes["found"], true);
    assert_eq!(notes["record_role"], "Assistant");
    assert!(notes["evidence_warning"]
        .as_str()
        .unwrap()
        .contains("not original evidence"));
    let missing = tool
        .execute(json!({"file_path":"evidence.jsonl","find":"not present"}))
        .await
        .unwrap();
    assert_eq!(missing["found"], false);
    assert_eq!(missing["match_count"], 0);
    assert_eq!(missing["last_match_offset"], Value::Null);
    assert_eq!(missing["total_records"], 2);
    assert!(missing["search_scope"]
        .as_str()
        .unwrap()
        .contains("/content"));
    assert!(automatic["content"]
        .as_str()
        .unwrap()
        .contains("instruction\nsecond line"));
    assert!(result["content"]
        .as_str()
        .unwrap()
        .contains("instruction\nsecond line"));
    assert!(result["content"].as_str().unwrap().chars().count() <= READ_PAGE_CHARS);
    let next = result["next_offset"].as_u64().unwrap();
    let absent = tool.execute(json!({"file_path":"evidence.jsonl","jsonl_record":2,"find":"Original 🔎","offset":next})).await.unwrap();
    assert_eq!(absent["found"], false);
    assert_eq!(absent["content"], "");
    let page = tool
        .execute(json!({"file_path":"evidence.jsonl","jsonl_record":2,"offset":5000}))
        .await
        .unwrap();
    assert!(page["content"].as_str().unwrap().starts_with("Original 🔎"));
    let first = tool
        .execute(json!({"file_path":"evidence.jsonl"}))
        .await
        .unwrap();
    assert_eq!(
        first["content"],
        dump.chars().take(READ_PAGE_CHARS).collect::<String>()
    );
    assert_eq!(first["next_offset"], READ_PAGE_CHARS);
    assert_eq!(first["truncated"], true);
    for args in [
        json!({"file_path":"../outside","offset":0}),
        json!({"file_path":"evidence.jsonl","jsonl_record":0}),
        json!({"file_path":"evidence.jsonl","offset":-1}),
        json!({"file_path":"evidence.jsonl","find":""}),
        json!({"file_path":"evidence.jsonl","json_pointer":"/content"}),
        json!({"file_path":"evidence.jsonl","jsonl_record":2,"json_pointer":"/missing"}),
    ] {
        assert!(tool.execute(args).await.is_err());
    }
    // An absolute path inside the project reads like the relative one.
    let absolute = tool
        .execute(json!({"file_path":root.join("evidence.jsonl"),"offset":0}))
        .await
        .unwrap();
    assert_eq!(absolute["content"], first["content"]);
    assert!(hook
        .seen()
        .iter()
        .all(|a| a.tool == "read_file" && a.risk == RiskLevel::Read));
    assert_eq!(
        fs::read_to_string(root.join("evidence.jsonl")).unwrap(),
        dump
    );
    fs::write(root.join("tool.jsonl"), json!({"role":"Tool","content":"","message_type":{"ToolResult":[{"function":{"arguments":"Quoted 🔎 output\nverbatim"}}]}}).to_string()).unwrap();
    let tool_result = tool.execute(json!({"file_path":"tool.jsonl","jsonl_record":1,"json_pointer":"/message_type/ToolResult/0/function/arguments"})).await.unwrap();
    assert_eq!(tool_result["record_role"], "Tool");
    assert_eq!(tool_result["content"], "Quoted 🔎 output\nverbatim");
}

#[tokio::test]
async fn bounded_find_exposes_later_unicode_matches() {
    let (_tmp, root) = rooted_case();
    fs::write(root.join("events.txt"), "é🔎 plan; later é🔎 done").unwrap();
    let tools = boxed_tools(&root, Arc::new(RecordingHook::new(Approval::AllowOnce))).unwrap();
    let tool = find_tool(&tools, "read_file");
    let first = tool
        .execute(json!({"file_path":"events.txt","find":"é🔎"}))
        .await
        .unwrap();
    assert_eq!(first["match_count"], 2);
    assert_eq!(first["match_offset"], 0);
    assert_eq!(first["last_match_offset"], 15);
    let later = tool
        .execute(json!({"file_path":"events.txt","find":"é🔎","offset":first["last_match_offset"]}))
        .await
        .unwrap();
    assert_eq!(later["match_count"], 1);
    assert_eq!(later["match_offset"], 15);
    assert_eq!(later["last_match_offset"], 15);
    let absent = tool
        .execute(json!({"file_path":"events.txt","find":"é🔎","offset":later["next_offset"]}))
        .await
        .unwrap();
    assert_eq!(absent["match_count"], 0);
    assert_eq!(absent["last_match_offset"], Value::Null);
    assert_eq!(absent["found"], false);
}

#[tokio::test]
async fn gated_delete_denied_under_deny_all() {
    let (_tmp, root) = rooted_case();
    let hook = Arc::new(RecordingHook::new(Approval::Deny));
    let tools = boxed_tools(&root, hook.clone()).unwrap();
    let delete = find_tool(&tools, "delete_file");

    // Plain file delete.
    fs::write(root.join("victim.txt"), "x").unwrap();
    let err = delete
        .execute(json!({ "path": "victim.txt", "recursive": false }))
        .await
        .unwrap_err();
    assert!(err.to_string().to_lowercase().contains("approv"), "{err}");
    assert!(root.join("victim.txt").exists());

    // Recursive directory delete flows through the same Destructive gate.
    fs::create_dir(root.join("dir")).unwrap();
    fs::write(root.join("dir/inner.txt"), "x").unwrap();
    let err = delete
        .execute(json!({ "path": "dir", "recursive": true }))
        .await
        .unwrap_err();
    assert!(err.to_string().to_lowercase().contains("approv"), "{err}");
    assert!(root.join("dir/inner.txt").exists());

    let seen = hook.seen();
    assert_eq!(seen.len(), 2);
    for action in &seen {
        assert_eq!(action.tool, "delete_file");
        assert_eq!(action.risk, RiskLevel::Destructive);
    }
}

#[tokio::test]
async fn git_read_obeys_custom_deny_policy() {
    let temporary = tempdir().unwrap();
    let policy = crate::configuration::ApprovalPolicy {
        default: crate::configuration::PolicyAction::Allow,
        rules: vec![crate::configuration::ApprovalRule {
            command: None,
            tool: "git".into(),
            action: crate::configuration::PolicyAction::Deny,
            risk: Some("read".into()),
        }],
        ..Default::default()
    };
    let approvals = crate::configuration::configured_hook(
        crate::configuration::ApprovalMode::Custom,
        policy,
        allow_all(),
    );
    let git = GitTool::new(temporary.path().to_owned(), approvals);
    let error = git.execute(json!({"args":["status"]})).await.unwrap_err();
    assert!(error.to_string().contains("denied"));
}

#[tokio::test]
async fn yolo_accesses_external_files_preserving_relative_paths() {
    let temporary = tempdir().unwrap();
    let root = temporary.path().join("project");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("inside.txt"), "inside").unwrap();
    fs::write(temporary.path().join("outside.txt"), "outside").unwrap();
    let tools = boxed_tools_with_mode(&root, allow_all(), true).unwrap();
    let read = tools.iter().find(|t| t.name() == "read_file").unwrap();
    let inside = read
        .execute(json!({"file_path":"inside.txt"}))
        .await
        .unwrap();
    assert_eq!(inside["content"], "inside");
    let outside = read
        .execute(json!({"file_path":"../outside.txt"}))
        .await
        .unwrap();
    assert_eq!(outside["content"], "outside");
    let shell = tools.iter().find(|t| t.name() == "shell").unwrap();
    let result = shell
        .execute(json!({"command":"pwd","cwd":".."}))
        .await
        .unwrap();
    assert!(result["success"].as_bool().unwrap());
    let patch = tools.iter().find(|t| t.name() == "apply_patch").unwrap();
    patch.execute(json!({"patch":"--- a/../outside.txt\n+++ b/../outside.txt\n@@ -1 +1 @@\n-outside\n+changed\n"})).await.unwrap();
    assert_eq!(
        fs::read_to_string(temporary.path().join("outside.txt")).unwrap(),
        "changed\n"
    );
    let custom = boxed_tools_with_mode(&root, allow_all(), false).unwrap();
    assert!(custom
        .iter()
        .find(|t| t.name() == "read_file")
        .unwrap()
        .execute(json!({"file_path":"../outside.txt"}))
        .await
        .is_err());
}

#[test]
fn gated_tool_delegates_metadata() {
    let (_tmp, root) = rooted_case();
    let tools = boxed_tools(&root, allow_all()).unwrap();
    let gated = find_tool(&tools, "write_file");
    let inner = WriteFile::new_with_root_dir(root.to_string_lossy().into_owned());
    assert_eq!(gated.name(), inner.name());
    assert_eq!(gated.description(), inner.description());
    assert_eq!(gated.args_schema(), inner.args_schema());
    assert_eq!(gated.output_schema(), inner.output_schema());
    let debug = format!("{gated:?}");
    assert!(debug.contains("GatedTool"), "{debug}");
}

#[test]
fn shell_default_timeout_is_120s() {
    let (_tmp, root) = rooted_case();
    assert_eq!(SHELL_TIMEOUT_SECS, 120);
    let tool = ShellTool::new(root, allow_all());
    assert_eq!(tool.timeout(), Duration::from_secs(120));
    let tool = tool.with_timeout(Duration::from_secs(1));
    assert_eq!(tool.timeout(), Duration::from_secs(1));
}

/// Models often leave out `append`; the write runs once, as a replace, after one
/// approval (the toolkit used to refuse it after approval, asking again on retry).
#[tokio::test]
async fn write_file_without_append_replaces_the_file_after_one_approval() {
    let (_tmp, root) = rooted_case();
    let hook = Arc::new(RecordingHook::new(Approval::AllowOnce));
    let tools = boxed_tools(&root, hook.clone()).unwrap();
    let write = tools
        .iter()
        .find(|tool| tool.name() == "write_file")
        .unwrap();
    fs::write(root.join("numbers.txt"), "old\n").unwrap();
    write
        .execute(json!({"file_path": "numbers.txt", "content": "4217\n12\n30\n"}))
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(root.join("numbers.txt")).unwrap(),
        "4217\n12\n30\n"
    );
    assert_eq!(hook.seen().len(), 1);
}

/// The toolkit's append returns before tokio's buffered write lands; the
/// gated call must return only once the bytes are on disk, even when the
/// machine is busy (many parallel appenders keep the blocking pool full).
#[test]
fn gated_append_is_durable_when_the_call_returns() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (_tmp, root) = rooted_case();
        let tools = Arc::new(boxed_tools(&root, allow_all()).unwrap());
        let mut jobs = Vec::new();
        for worker in 0..8 {
            let tools = Arc::clone(&tools);
            let root = root.clone();
            jobs.push(tokio::spawn(async move {
                let write = tools
                    .iter()
                    .find(|tool| tool.name() == "write_file")
                    .unwrap();
                for round in 0..40 {
                    let name = format!("w{worker}.txt");
                    let line = format!("{round}\n");
                    write
                        .execute(json!({"file_path": name, "content": line, "append": true}))
                        .await
                        .unwrap();
                    let text = fs::read_to_string(root.join(&name)).unwrap();
                    assert_eq!(
                        text.lines().count(),
                        round + 1,
                        "append {round} of {name} not on disk: {text:?}"
                    );
                }
            }));
        }
        for job in jobs {
            job.await.unwrap();
        }
    });
}

#[tokio::test]
async fn shell_command_with_whole_command_line_explains_how_to_split_it() {
    let (_tmp, root) = rooted_case();
    let hook = Arc::new(RecordingHook::new(Approval::AllowOnce));
    let tool = ShellTool::new(root, hook.clone());
    let err = tool
        .execute(json!({"command": "python3 -m http.server 8798", "args": []}))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains(r#"command: "python3", args: ["-m","http.server","8798"]"#),
        "{err}"
    );
    // Existing args stay after the split tokens.
    let err = tool
        .execute(json!({"command": "which python3", "args": ["-a"]}))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains(r#"command: "which", args: ["python3","-a"]"#),
        "{err}"
    );
    // Never auto-split: approval saw the exact argv that was sent.
    let seen = hook.seen();
    assert_eq!(seen[0].command, vec!["python3 -m http.server 8798"]);
    assert_eq!(seen[1].command, vec!["which python3", "-a"]);
}

#[test]
fn spawn_failure_hints_only_for_missing_programs_with_whitespace() {
    let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
    let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    assert!(!spawn_failure("nonexistent", &[], &missing).contains("Retry"));
    assert!(!spawn_failure("a b", &[], &denied).contains("Retry"));
    assert!(spawn_failure("a  b\tc", &[], &missing).contains(r#"args: ["b","c"]"#));
    // A missing path with spaces is not a command line: no wrong split.
    let spaced = spawn_failure("/Applications/My App/bin/x", &[], &missing);
    assert!(!spaced.contains("Retry exactly"), "{spaced}");
    assert!(spaced.contains("contains spaces"), "{spaced}");
}

#[test]
fn argv_repairs_the_shapes_models_send() {
    let v = |words: &[&str]| words.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
    // Program repeated as the first argument, plainly or as a path.
    assert_eq!(
        normalize_argv(v(&["python3", "python3", "x.py"])),
        v(&["python3", "x.py"])
    );
    assert_eq!(
        normalize_argv(v(&["python3", "/usr/bin/python3", "x.py"])),
        v(&["python3", "x.py"])
    );
    assert_eq!(
        normalize_argv(v(&["/usr/bin/git", "git", "status"])),
        v(&["/usr/bin/git", "status"])
    );
    // Empty command with the program in args.
    assert_eq!(
        normalize_argv(v(&["", "grep", "-c", "x", "f"])),
        v(&["grep", "-c", "x", "f"])
    );
    // Ordinary commands and echo of its own name are left alone.
    assert_eq!(normalize_argv(v(&["git", "status"])), v(&["git", "status"]));
    assert_eq!(normalize_argv(v(&["echo", "echo"])), v(&["echo", "echo"]));
    assert_eq!(
        normalize_argv(v(&["python3", "-c", "print(1)"])),
        v(&["python3", "-c", "print(1)"])
    );
    // The approval argv and the executed argv agree.
    let raw = serde_json::json!({"command": "", "args": ["python3", "python3", "x.py"]});
    assert_eq!(shell_argv(&raw), v(&["python3", "x.py"]));
    assert!(shell_argv(&serde_json::json!({"command": "", "args": []})).is_empty());
}

#[tokio::test]
async fn refusals_tell_the_agent_not_to_work_around_them() {
    let project = tempfile::tempdir().unwrap();
    let denied = ShellTool::new(project.path().to_owned(), Arc::new(DenyAllHook))
        .execute(serde_json::json!({"command": "ls"}))
        .await
        .unwrap_err()
        .to_string();
    assert!(denied.contains(REFUSAL_NOTE), "{denied}");
    let git = GitTool::new(project.path().to_owned(), Arc::new(AllowAllHook))
        .execute(serde_json::json!({"args": ["reset", "--hard"]}))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        git.contains("allowlist") && git.contains(REFUSAL_NOTE),
        "{git}"
    );
}

#[tokio::test]
async fn archive_selectors_on_plain_files_are_ignored_with_a_note() {
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("story.txt"),
        "Odile walked to the mill.",
    )
    .unwrap();
    let tools = boxed_tools(project.path(), Arc::new(AllowAllHook)).unwrap();
    let read = tools
        .iter()
        .find(|tool| tool.name() == "read_file")
        .unwrap();
    let result = read
            .execute(serde_json::json!({"file_path": "story.txt", "jsonl_record": 1, "json_pointer": "/content"}))
            .await
            .unwrap();
    assert!(
        result["content"].as_str().unwrap().contains("Odile"),
        "{result}"
    );
    assert!(result["selectors_ignored"].is_string(), "{result}");
    let plain = read
        .execute(serde_json::json!({"file_path": "story.txt"}))
        .await
        .unwrap();
    assert!(plain.get("selectors_ignored").is_none());
}

mod shell_args_props {
    use super::*;
    use proptest::prelude::*;

    fn parsed(args: Value) -> Result<ShellArgs, ToolCallError> {
        parse_tool_args("shell", json!({"command": "python3", "args": args}))
    }

    #[test]
    fn args_sent_as_json_text_mean_the_same_list() {
        // What rules and prompts read matches what runs.
        assert_eq!(
            shell_argv(&json!({"command": "rm", "args": r#"["-rf", "x"]"#})),
            ["rm", "-rf", "x"]
        );
        assert_eq!(
            shell_risk(&shell_argv(
                &json!({"command": "rm", "args": r#"["-rf", "x"]"#})
            )),
            shell_risk(&shell_argv(&json!({"command": "rm", "args": ["-rf", "x"]})))
        );
        let list = parsed(json!(["-c", "print(1)"])).ok().unwrap().args;
        let text = parsed(json!(r#"["-c", "print(1)"]"#)).ok().unwrap().args;
        assert_eq!(list, text);
        assert_eq!(text, ["-c", "print(1)"]);
        assert!(
            parsed(json!("-c print(1)")).is_err(),
            "never split by guessing"
        );
        assert!(parsed(json!(r#"{"a": 1}"#)).is_err());
        assert!(parsed(json!([1, 2])).is_err());
    }

    proptest! {
        /// Any list survives being sent as JSON text unchanged.
        #[test]
        fn any_list_survives_json_text(list in proptest::collection::vec(".*", 0..6)) {
            let text = serde_json::to_string(&list).unwrap();
            prop_assert_eq!(parsed(json!(text)).ok().unwrap().args, list);
        }

        /// Any text is either a list of strings or an error, never a panic.
        #[test]
        fn any_text_parses_or_is_refused(text in ".*") {
            let _ = parsed(json!(text));
        }
    }
}

mod patch_props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// With distinct lines, a hunk whose header counts and start line are wrong still
        /// edits exactly the intended span and nothing else.
        #[test]
        fn miscounted_misplaced_hunks_edit_exactly_their_span(
            len in 3usize..30,
            start in 0usize..30,
            take in 1usize..4,
            replacement in proptest::collection::vec("[a-z]{1,6}", 0..4),
            declared_start in 0usize..60,
            declared_count in 0usize..9,
        ) {
            let old: Vec<String> = (0..len).map(|n| format!("line-{n}")).collect();
            let start = start % len;
            let end = (start + take).min(len);
            let mut body = Vec::new();
            if start > 0 { body.push(format!(" {}", old[start - 1])); }
            body.extend(old[start..end].iter().map(|l| format!("-{l}")));
            body.extend(replacement.iter().map(|l| format!("+new-{l}")));
            if end < len { body.push(format!(" {}", old[end])); }
            let mut lines = vec![format!("@@ -{},{declared_count} +1,{declared_count} @@", declared_start + 1)];
            lines.extend(body);
            let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
            let (hunk, _) = parse_hunk(&refs, 0).unwrap();
            let applied = apply_hunks(&old, &[hunk], "f").unwrap();
            // A stray closing marker changes nothing.
            let mut marked = refs.clone();
            marked.push("*** End Patch");
            let (marked_hunk, _) = parse_hunk(&marked, 0).unwrap();
            prop_assert_eq!(&apply_hunks(&old, &[marked_hunk], "f").unwrap(), &applied);
            let mut expected = old[..start].to_vec();
            expected.extend(replacement.iter().map(|l| format!("new-{l}")));
            expected.extend_from_slice(&old[end..]);
            prop_assert_eq!(applied, expected);
        }
    }
}

#[test]
fn shell_args_sent_as_text_survive_raw_line_breaks_and_a_bad_list_says_what_to_do() {
    // Raw line breaks inside a JSON string are the commonest way this text breaks.
    let script = "[\"-c\", \"import json\nprint(1)\ttwo\"]";
    assert_eq!(
        argv_text(script).unwrap(),
        vec!["-c".to_owned(), "import json\nprint(1)\ttwo".to_owned()]
    );
    // Line breaks between items are only whitespace.
    assert_eq!(argv_text("[\"a\",\n \"b\"]").unwrap(), vec!["a", "b"]);
    // An escaped quote doesn't end the string.
    assert_eq!(argv_text("[\"a\\\"\nb\"]").unwrap(), vec!["a\"\nb"]);
    // A pipe tacked on is refused, not guessed at, and the error shows the way.
    assert!(argv_text("[\"find\", \".\"] | head").is_none());
    assert!(crate::prompts::TOOL_SHELL_ARGS_INVALID.contains("\"sh\""));
    let parsed: std::result::Result<ShellArgs, _> =
        serde_json::from_value(json!({"command":"find","args":"[\"find\", \".\"] | head"}));
    assert!(parsed.unwrap_err().to_string().contains("\"-c\""));
}

#[test]
fn list_arguments_sent_as_a_string_are_read_as_the_list() {
    let repaired = repair_list_arguments(
        "shell",
        r#"{"command":"grep","args":"[\"-L\", \"x\", \"out\"]","cwd":"."}"#,
    );
    let value: Value = serde_json::from_str(&repaired).unwrap();
    assert_eq!(value["args"], json!(["-L", "x", "out"]));
    assert_eq!(value["cwd"], json!("."));
    assert_eq!(shell_argv(&value), ["grep", "-L", "x", "out"]);
    let git = repair_list_arguments("git", r#"{"args":"[\"status\"]"}"#);
    assert_eq!(
        serde_json::from_str::<Value>(&git).unwrap()["args"],
        json!(["status"])
    );
    // Anything that is not a string holding a list of strings is left for the usual error.
    for unchanged in [
        r#"{"command":"grep","args":["-L","x"]}"#,
        r#"{"command":"grep","args":"-L x"}"#,
        r#"{"command":"grep","args":"[1, 2]"}"#,
        r#"{"command":"grep","args":"{\"a\":1}"}"#,
        "not json",
    ] {
        assert_eq!(repair_list_arguments("shell", unchanged), unchanged);
    }
    let other = r#"{"args":"[\"a\"]"}"#;
    assert_eq!(repair_list_arguments("read_file", other), other);
    // The person approves what will run, so a repaired script reads the same both ways.
    let script = repair_list_arguments(
        "shell",
        &json!({"command":"python3","args":"[\"-c\", \"a\nb\"]"}).to_string(),
    );
    let value: Value = serde_json::from_str(&script).unwrap();
    assert_eq!(shell_argv(&value), ["python3", "-c", "a\nb"]);
    // Rules read the raw call too, and must see what the tool would run.
    let raw = json!({"command":"sh","args":"[\"-c\", \"rm -rf x\ny\"]"});
    assert_eq!(shell_argv(&raw), ["sh", "-c", "rm -rf x\ny"]);
}

mod repair_list_arguments_props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// A list sent properly passes through unchanged in meaning, and the same list
        /// sent as a JSON string comes out identical to it.
        #[test]
        fn a_list_and_the_same_list_as_a_string_agree(
            command in "[a-z]{1,8}",
            list in proptest::collection::vec(any::<String>(), 0..6),
        ) {
            let proper = json!({"command": command, "args": list}).to_string();
            let as_string = json!({
                "command": command,
                "args": serde_json::to_string(&list).unwrap(),
            }).to_string();
            let from_proper: Value = serde_json::from_str(&repair_list_arguments("shell", &proper)).unwrap();
            let from_string: Value = serde_json::from_str(&repair_list_arguments("shell", &as_string)).unwrap();
            prop_assert_eq!(&from_proper, &serde_json::from_str::<Value>(&proper).unwrap());
            prop_assert_eq!(from_string, from_proper);
        }

        /// Arbitrary input never panics, and a result that changed is valid JSON whose
        /// `args` is a list of strings.
        #[test]
        fn arbitrary_arguments_are_safe(tool in "shell|git|read_file", arguments in any::<String>()) {
            let repaired = repair_list_arguments(&tool, &arguments);
            if repaired != arguments {
                let value: Value = serde_json::from_str(&repaired).unwrap();
                prop_assert!(value["args"].as_array().unwrap().iter().all(Value::is_string));
            }
        }
    }
}

mod normalize_argv_props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Repair only ever drops one leading duplicate or an empty program: the rest of
        /// the command keeps its order and nothing is added.
        #[test]
        fn repair_only_removes_a_leading_duplicate(
            argv in proptest::collection::vec(prop_oneof![
                Just(String::new()), Just("python3".to_owned()), Just("/usr/bin/python3".to_owned()),
                Just("echo".to_owned()), "[a-z./-]{1,8}",
            ], 1..6),
        ) {
            let repaired = normalize_argv(argv.clone());
            prop_assert!(argv.len() - repaired.len() <= 2);
            // An order-preserving subsequence of the input.
            let mut rest = argv.iter();
            prop_assert!(repaired.iter().all(|word| rest.any(|original| original == word)));
            // A real program is kept as the program.
            if !argv[0].trim().is_empty() {
                prop_assert_eq!(&repaired[0], &argv[0]);
            }
            // Everything after the program and its possible duplicate is untouched.
            let start = if argv[0].trim().is_empty() { 3 } else { 2 };
            if argv.len() > start {
                let tail = &argv[start..];
                prop_assert_eq!(&repaired[repaired.len() - tail.len()..], tail);
            }
        }
    }
}

mod project_relative_props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Whatever absolute path a model sends, a mapped result never climbs out of the
        /// project and always names the same place inside it.
        #[test]
        fn mapped_paths_stay_inside_the_project(
            parts in proptest::collection::vec(prop_oneof![
                Just("..".to_owned()), Just(".".to_owned()), "[a-z]{1,6}",
            ], 0..6),
        ) {
            let project = tempfile::tempdir().unwrap();
            let base = project.path().canonicalize().unwrap();
            std::fs::create_dir_all(base.join("a/b")).unwrap();
            let candidate = parts.iter().fold(base.clone(), |path, part| path.join(part));
            if let Some(relative) = project_relative(&base, &candidate) {
                prop_assert!(!Path::new(&relative).components().any(|c| c == std::path::Component::ParentDir));
                prop_assert!(!Path::new(&relative).is_absolute());
                let mapped = base.join(&relative);
                let resolved = mapped.canonicalize().unwrap_or(mapped);
                prop_assert!(resolved.starts_with(&base), "{} -> {}", candidate.display(), resolved.display());
            }
        }
    }
}

mod spawn_failure_props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn never_panics_and_hint_round_trips(
            program in "[ -~]{0,20}",
            args in proptest::collection::vec("[ -~]{0,6}", 0..4),
        ) {
            let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
            let message = spawn_failure(&program, &args, &missing);
            prop_assert!(message.starts_with("failed to start"));
            if program.contains(char::is_whitespace)
                && program.split_whitespace().next().is_some_and(|head| !head.contains(['/', '\\']))
            {
                let tail = message.split("args: ").nth(1).expect("hint present");
                let argv: Vec<String> = serde_json::from_str(tail).unwrap();
                let mut expected: Vec<String> =
                    program.split_whitespace().skip(1).map(str::to_owned).collect();
                expected.extend(args.iter().cloned());
                prop_assert_eq!(argv, expected);
            }
        }
    }
}

mod commands;

/// Commands under a real sandbox backend on this machine; every test is
/// skipped (with the reason) where no backend is usable.
#[cfg(unix)]
mod sandboxed;

#[test]
fn read_only_git_calls_are_reads_and_flags_that_write_or_run_programs_are_not() {
    let risk = |args: &[&str]| git_call_risk(&json!({ "args": args }));
    for args in [
        &["status", "--short"][..],
        &["diff", "--cached", "--stat", "--", "src/a.rs"],
        &["log", "--oneline", "-5", "--format=%h %s"],
        &["log", "-n", "3", "--since=2.weeks"],
        &["show", "HEAD~1", "--name-only"],
        &["log", "--oneline", "main..feature"],
        &["diff", "notes..v2.txt"],
    ] {
        assert_eq!(risk(args), RiskLevel::Read, "{args:?}");
    }
    for args in [
        &["diff", "--output=/tmp/out.patch"][..],
        &["log", "-p", "--ext-diff"],
        &["show", "--textconv", "HEAD"],
        &["diff", "--no-index", "/etc/hosts", "x"],
        &["commit", "-m", "a b"],
        &["add", "."],
        &["status", "-"],
        // Paths outside the project: `git diff` compares them without `--no-index`.
        &["diff", "/etc/hosts", "/tmp/x"],
        &["diff", "../outside.txt", "inside.txt"],
        &["show", "~/notes"],
        &["diff", "src/../../outside.txt"],
        &["diff", "C:\\Windows\\win.ini", "x"],
        &["diff", "\\\\server\\share\\file", "x"],
        &["diff", "\\Windows\\win.ini", "x"],
    ] {
        assert_eq!(risk(args), RiskLevel::Write, "{args:?}");
    }
    for args in [&["checkout", "--", "."][..], &["branch", "-D", "old"]] {
        assert_eq!(risk(args), RiskLevel::Destructive, "{args:?}");
    }
    assert_eq!(
        git_call_risk(&json!({})),
        RiskLevel::Write,
        "unreadable arguments"
    );
}
