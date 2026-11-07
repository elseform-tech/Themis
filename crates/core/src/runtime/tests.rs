fn ledger_text(
    messages: &[ChatMessage],
    bytes: usize,
    snapshot: Option<&std::path::Path>,
) -> String {
    super::ledger::render(
        messages,
        messages.len(),
        super::ledger::Budget {
            total: bytes,
            actions: bytes,
        },
        snapshot,
    )
}
use super::*;
use crate::tools::FnHook;
use std::sync::atomic::{AtomicUsize, Ordering};

fn action(tool: &str) -> ToolAction {
    ToolAction {
        display: None,
        command: Vec::new(),
        tool: tool.to_owned(),
        summary: "test".to_owned(),
        risk: RiskLevel::Write,
    }
}

/// A steered message reaches the model at the next request boundary, once, and in order;
/// one that arrives while the model writes its final answer keeps the run going.
/// A final answer long enough to be the turn's summary.
const ADJUSTED: &str = "Adjusted the file to use tabs.\nNothing else changed.";

async fn recovered_original_excerpt_fixture() -> (tempfile::TempDir, Vec<ChatMessage>) {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join(".themis/context/test");
    std::fs::create_dir_all(&folder).unwrap();
    let source = folder.join("snapshot-original.jsonl");
    let records = [
        serde_json::json!({"role":"Tool","evidence_kind":"original","message_type":{"ToolResult":[{"evidence_kind":"original","function":{"arguments":serde_json::json!({"exit_code":1,"stdout":format!("FAIL: RETAIN_PENDING_ASSERTION. Newer pending must suppress older posted. {}", "x".repeat(3000))}).to_string()}}]}}),
        serde_json::json!({"role":"Tool","evidence_kind":"recovery","message_type":{"ToolResult":[{"evidence_kind":"recovery","function":{"arguments":serde_json::json!({"stdout":format!("RECURSIVE_COPY_MUST_BE_EXCLUDED {}", "r".repeat(3000))}).to_string()}}]}}),
        serde_json::json!({"role":"Assistant","evidence_kind":"navigation","content":"NAVIGATION_COPY_MUST_BE_EXCLUDED"}),
    ];
    std::fs::write(
        &source,
        records.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    let tools =
        crate::tools::boxed_tools(root.path(), Arc::new(crate::tools::AllowAllHook)).unwrap();
    let reader = tools
        .iter()
        .find(|tool| tool.name() == "read_file")
        .unwrap();
    let mut messages = Vec::new();
    for (id, record, pointer) in [
        (
            "original",
            1,
            "/message_type/ToolResult/0/function/arguments",
        ),
        (
            "duplicate",
            1,
            "/message_type/ToolResult/0/function/arguments",
        ),
        (
            "recursive",
            2,
            "/message_type/ToolResult/0/function/arguments",
        ),
        ("navigation", 3, "/content"),
    ] {
        let mut args = serde_json::json!({"file_path":".themis/context/test/snapshot-original.jsonl","jsonl_record":record,"json_pointer":pointer,"decode_tool_payload":true});
        if record == 1 {
            args["find"] = "x".repeat(2100).into();
        }
        let output = reader.execute(args.clone()).await.unwrap();
        let call = ToolCall {
            id: id.into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: args.to_string(),
            },
        };
        messages.push(ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::ToolUse(vec![call.clone()]),
            content: String::new(),
        });
        let mut result = call;
        result.function.arguments = output.to_string();
        messages.push(ChatMessage {
            role: ChatRole::Tool,
            message_type: MessageType::ToolResult(vec![result]),
            content: String::new(),
        });
    }
    (root, messages)
}

#[derive(Debug)]
struct StubTool(Option<Arc<AtomicBool>>);

#[autoagents::async_trait]
impl autoagents::core::tool::ToolRuntime for StubTool {
    async fn execute(
        &self,
        _args: serde_json::Value,
    ) -> Result<serde_json::Value, autoagents::core::tool::ToolCallError> {
        if let Some(stopped) = &self.0 {
            stopped.store(true, Ordering::SeqCst);
        }
        Ok(serde_json::json!({"ok": true}))
    }
}

impl ToolT for StubTool {
    fn name(&self) -> &str {
        "stub"
    }

    fn description(&self) -> &str {
        "test stub"
    }

    fn args_schema(&self) -> serde_json::Value {
        serde_json::json!({})
    }
}

mod context_and_retries;
mod evidence_and_dispatch;
mod nudges_and_compaction;
