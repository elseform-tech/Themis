use super::*;

// ---------------------------------------------------------------------------
// Approval-gating wrapper for third-party tools
// ---------------------------------------------------------------------------

/// Builds a one-line summary for a gated tool call.
///
/// Receives the inner tool's name and the raw JSON args; the result becomes
/// the [`ToolAction::summary`] shown to the approval hook.
pub type SummaryFn = fn(&str, &Value) -> String;

/// Wraps a third-party [`ToolT`] whose `execute` never consults our
/// [`ApprovalHook`] so every call is approval-gated first.
///
/// Metadata delegates to the inner tool except `read_file`, whose schema and
/// description also expose bounded reads. Tool names stay unchanged for skill
/// filters and LLM bindings. `execute` builds a [`ToolAction`] from the
/// inner name, `risk`, and `summarize`, consults the hook (`AllowOnce` /
/// `AllowAlways` proceed; `Deny` fails with an approval-mentioning error),
/// then delegates to the inner tool.
///
/// [`boxed_tools`] wraps every `autoagents-toolkit` filesystem tool in this;
/// the custom [`ShellTool`], [`ApplyPatchTool`], and [`GitTool`] gate
/// internally and are intentionally *not* double-wrapped.
pub struct GatedTool {
    inner: Box<dyn ToolT>,
    approvals: Arc<dyn ApprovalHook>,
    risk: RiskLevel,
    summarize: SummaryFn,
    relative_root: Option<PathBuf>,
    /// Directory relative paths resolve against; used to confirm appends landed.
    base_dir: Option<PathBuf>,
    /// Paths the sandbox protects from Themis' own file tools.
    guard: Option<Arc<crate::sandbox::FileGuard>>,
}

impl GatedTool {
    /// Wrap `inner` with `risk` and the default filesystem summary builder.
    pub fn new(inner: Box<dyn ToolT>, approvals: Arc<dyn ApprovalHook>, risk: RiskLevel) -> Self {
        Self {
            inner,
            approvals,
            risk,
            summarize: summarize_fs_call,
            relative_root: None,
            base_dir: None,
            guard: None,
        }
    }

    /// Wrap `inner` with `risk` and a custom summary builder.
    pub fn with_summary(
        inner: Box<dyn ToolT>,
        approvals: Arc<dyn ApprovalHook>,
        risk: RiskLevel,
        summarize: SummaryFn,
    ) -> Self {
        Self {
            inner,
            approvals,
            risk,
            summarize,
            relative_root: None,
            base_dir: None,
            guard: None,
        }
    }

    /// Preserve project-relative paths when the underlying tool can access the filesystem.
    pub fn with_relative_root(mut self, root: PathBuf) -> Self {
        self.relative_root = Some(root);
        self
    }

    /// Directory that relative tool paths resolve against.
    #[must_use]
    pub fn with_base_dir(mut self, dir: PathBuf) -> Self {
        self.base_dir = Some(dir);
        self
    }

    /// Refuses reads of secret locations and writes to protected folders.
    #[must_use]
    pub fn with_guard(mut self, guard: Option<Arc<crate::sandbox::FileGuard>>) -> Self {
        self.guard = guard;
        self
    }

    /// The risk level this wrapper gates under.
    pub fn risk(&self) -> RiskLevel {
        self.risk
    }

    /// Applies the sandbox file guard to every path argument of `args`.
    fn check_guard(&self, name: &str, args: &Value) -> Result<(), ToolCallError> {
        let Some(guard) = &self.guard else {
            return Ok(());
        };
        for key in [
            "path",
            "file_path",
            "directory_path",
            "source_path",
            "destination_path",
            "directory",
        ] {
            let Some(path) = args.get(key).and_then(Value::as_str) else {
                continue;
            };
            let path = Path::new(path);
            let path = match &self.base_dir {
                Some(base) if path.is_relative() => base.join(path),
                _ => path.to_path_buf(),
            };
            let read_only =
                self.risk == RiskLevel::Read || (name == "copy_file" && key == "source_path");
            let checked = if read_only {
                guard.check_read(&path)
            } else {
                guard.check_write(&path)
            };
            checked.map_err(|e| tool_error(format!("{name}: {e}")))?;
        }
        Ok(())
    }
}

/// Removes JSONL selectors from a `read_file` call on a file that is not JSON lines
/// (by extension: `.jsonl`/`.ndjson`/`.json` keep them). Returns whether any were removed.
fn strip_jsonl_selectors(args: &mut Value) -> bool {
    let jsonl = args
        .get("file_path")
        .and_then(Value::as_str)
        .and_then(|path| Path::new(path).extension())
        .is_some_and(|ext| matches!(ext.to_str(), Some("jsonl" | "ndjson" | "json")));
    let Some(fields) = args.as_object_mut().filter(|_| !jsonl) else {
        return false;
    };
    ["jsonl_record", "json_pointer", "decode_tool_payload"]
        .into_iter()
        .filter(|key| fields.remove(*key).is_some())
        .count()
        > 0
}

/// `path` relative to `base` when it is absolute and lies under `base`; `None` otherwise.
/// The path is resolved first (the file, or its parent for a new file), so `/var/..` vs
/// `/private/var/..` spellings match and a symlink pointing outside the project does not;
/// the caller's root-scoped tool still validates the relative result.
pub(super) fn project_relative(base: &Path, path: &Path) -> Option<String> {
    if !path.is_absolute() {
        return None;
    }
    let base = base.canonicalize().ok()?;
    let resolved = path.canonicalize().ok().or_else(|| {
        let parent = path.parent()?.canonicalize().ok()?;
        Some(parent.join(path.file_name()?))
    })?;
    let rest = resolved.strip_prefix(&base).ok()?;
    let rest = rest.to_string_lossy().into_owned();
    Some(if rest.is_empty() {
        ".".to_owned()
    } else {
        rest
    })
}

impl std::fmt::Debug for GatedTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatedTool")
            .field("tool", &self.inner.name())
            .field("risk", &self.risk)
            .finish_non_exhaustive()
    }
}

impl ToolT for GatedTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        if self.inner.name() == "read_file" {
            "Read a project text file. Returns up to 8000 characters from offset (Unicode characters, default 0) with next_offset when more remains. Example: {\"file_path\":\"src/main.py\"}. find searches an exact, case-sensitive text from offset and also returns match_count and last_match_offset. For saved chat archives (.jsonl) only: jsonl_record selects a one-based record, json_pointer its string field (default /content), and decode_tool_payload=true searches decoded tool arguments (keep it for later offsets); recall is usually easier for those. To search, count or analyse whole files, use approved shell/Python and print only the facts you need."
        } else {
            self.inner.description()
        }
    }

    fn args_schema(&self) -> Value {
        let mut schema = self.inner.args_schema();
        if self.inner.name() == "read_file" {
            schema["properties"]["jsonl_record"] = serde_json::json!({"type":"integer","minimum":1,"description":"Only for JSONL content, never ordinary source/text files. One-based record; omit for a plain file read"});
            schema["properties"]["json_pointer"] = serde_json::json!({"type":"string","description":"Only for a selected JSONL record, never ordinary source/text files; default /content, e.g. /message_type/ToolResult/0/function/arguments"});
            schema["properties"]["decode_tool_payload"] = serde_json::json!({"type":"boolean","default":false,"description":"For archived Tool arguments, decode the JSON envelope once and page/search labeled leaf values (e.g. stdout/content) instead of escaped JSON. Return this same flag on subsequent offsets. Default false preserves the exact raw field."});
            schema["properties"]["offset"] = serde_json::json!({"type":"integer","minimum":0,"description":"Start character for paging or searching; use returned next_offset to continue"});
            schema["properties"]["find"] = serde_json::json!({"type":"string","minLength":1,"description":"Exact case-sensitive literal; returns surrounding text and next_offset for the next match"});
        }
        schema
    }

    fn output_schema(&self) -> Option<Value> {
        self.inner.output_schema()
    }
}

#[async_trait]
impl ToolRuntime for GatedTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        // The toolkit reports file-system failures as the bare OS error ("Permission denied
        // (os error 13)"); name the paths the model gave, so the error says which file failed.
        let paths = PATH_KEYS
            .iter()
            .filter_map(|key| args.get(*key).and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(", ");
        self.execute_gated(args).await.map_err(|error| {
            if paths.is_empty() || !is_bare_os_error(&error) {
                return error;
            }
            ToolCallError::RuntimeError(format!("{paths}: {}", runtime_message(&error)).into())
        })
    }
}

/// Argument names that hold a path, in the order a failing call's paths are reported.
const PATH_KEYS: [&str; 6] = [
    "file_path",
    "path",
    "source_path",
    "destination_path",
    "directory_path",
    "directory",
];

fn runtime_message(error: &ToolCallError) -> String {
    match error {
        ToolCallError::RuntimeError(inner) => inner.to_string(),
        other => other.to_string(),
    }
}

/// An OS error that names no path (the standard library adds none to `io::Error`).
fn is_bare_os_error(error: &ToolCallError) -> bool {
    let message = runtime_message(error);
    message.contains("(os error ") && !message.contains('/') && !message.contains('\\')
}

impl GatedTool {
    async fn execute_gated(&self, args: Value) -> Result<Value, ToolCallError> {
        let mut args = args;
        if let Some(root) = &self.relative_root {
            for key in PATH_KEYS {
                if let Some(path) = args.get(key).and_then(Value::as_str) {
                    if !Path::new(path).is_absolute() {
                        args[key] = Value::String(root.join(path).to_string_lossy().into_owned());
                    }
                }
            }
        } else if let Some(base) = &self.base_dir {
            // Root-scoped tools take project-relative paths only. Models often pass the
            // absolute path they were shown (e.g. the evidence directory); an absolute path
            // inside the project becomes relative, and the toolkit still validates the rest
            // (traversal, symlinks). Paths outside the project are left to be refused.
            for key in PATH_KEYS {
                if let Some(relative) = args
                    .get(key)
                    .and_then(Value::as_str)
                    .and_then(|path| project_relative(base, Path::new(path)))
                {
                    args[key] = Value::String(relative);
                }
            }
        }
        let name = self.inner.name();
        // `append` is optional to the model (the schema default is a replace), but the
        // toolkit's argument type requires it: fill it in before approval, so an approved
        // write is not refused afterwards and approved again on retry.
        if name == "write_file" && args.get("append").is_none() && args.is_object() {
            args["append"] = Value::Bool(false);
        }
        let action = ToolAction {
            display: None,
            command: Vec::new(),
            tool: name.to_owned(),
            summary: (self.summarize)(name, &args),
            risk: self.risk,
        };
        check_call_permitted(self.approvals.as_ref(), &action, &args)?;
        self.check_guard(name, &args)?;
        if name == "read_file" {
            // Models often send the archive-only selectors on ordinary files; read the file
            // plainly instead of failing, and say so.
            let ignored = strip_jsonl_selectors(&mut args);
            let result = self.inner.execute(args.clone()).await?;
            let mut result = bounded_read_result(&args, result)?;
            if ignored {
                result["selectors_ignored"] = Value::String(
                    "jsonl_record/json_pointer/decode_tool_payload apply only to .jsonl files; read as plain text".into(),
                );
            }
            Ok(result)
        } else if name == "write_file" && args["append"].as_bool() == Some(true) {
            // The toolkit appends through tokio's buffered file and returns
            // without flushing, so the bytes can land after the call reports
            // success (a reader then sees an empty file). Wait for them.
            let target = args["file_path"].as_str().map(|path| {
                let path = Path::new(path);
                match &self.base_dir {
                    Some(base) if path.is_relative() => base.join(path),
                    _ => path.to_path_buf(),
                }
            });
            let before = target
                .as_ref()
                .and_then(|path| std::fs::metadata(path).ok())
                .map_or(0, |meta| meta.len());
            let result = self.inner.execute(args).await?;
            if let (Some(path), Some(added)) = (target, result["bytes_written"].as_u64()) {
                await_appended(&path, before.saturating_add(added)).await;
            }
            Ok(result)
        } else {
            self.inner.execute(args).await
        }
    }
}

/// Polls until `path` holds at least `expected_len` bytes (bounded at 10 s).
async fn await_appended(path: &Path, expected_len: u64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let len = tokio::fs::metadata(path).await.map_or(0, |meta| meta.len());
        if len >= expected_len || Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

pub(crate) fn context_recovery_call(name: &str, arguments: &str) -> bool {
    matches!(name, "read_file" | "list_dir" | "shell" | "search_file")
        && arguments.replace('\\', "/").contains(".themis/context/")
}

/// True when a `recall` result carries a hit from an attachment original.
///
/// Snapshot and history-source hits are copies of saved evidence (recovery);
/// attachment text is acquired content that no other record holds in this form,
/// so such a result is an ordinary observation.
pub(crate) fn recall_result_has_attachment(result: &str) -> bool {
    serde_json::from_str::<Value>(result).is_ok_and(|value| {
        value["kind"] == "attachment"
            || value["hits"]
                .as_array()
                .is_some_and(|hits| hits.iter().any(|hit| hit["kind"] == "attachment"))
    })
}
fn archive_field_kind<'a>(record: &'a Value, pointer: &str) -> &'a str {
    pointer
        .strip_prefix("/message_type/ToolResult/")
        .and_then(|tail| tail.split('/').next()?.parse::<usize>().ok())
        .and_then(|index| record["message_type"]["ToolResult"][index]["evidence_kind"].as_str())
        .or_else(|| record["evidence_kind"].as_str())
        .unwrap_or("legacy")
}

fn decoded_archive_field(
    record: &Value,
    pointer: &str,
    decode_tool_payload: bool,
) -> Option<String> {
    let text = record.pointer(pointer)?.as_str()?;
    if !decode_tool_payload || record["role"] != "Tool" || !pointer.ends_with("/function/arguments")
    {
        return Some(text.to_owned());
    }
    fn leaves(value: &Value, path: &str, output: &mut String) {
        match value {
            Value::Object(fields) => {
                for (key, value) in fields {
                    leaves(value, &format!("{path}/{key}"), output);
                }
            }
            Value::Array(items) => {
                for (index, value) in items.iter().enumerate() {
                    leaves(value, &format!("{path}/{index}"), output);
                }
            }
            _ => {
                output.push_str(&format!(
                    "{path}:\n{}\n",
                    value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_owned)
                ));
            }
        }
    }
    // Decode the tool envelope once. Nested JSON-looking source text is still source text.
    match serde_json::from_str::<Value>(text) {
        Ok(value) => {
            let mut output = String::new();
            leaves(&value, "", &mut output);
            Some(output)
        }
        Err(_) => Some(text.to_owned()),
    }
}

/// Bound existing authorized reads; path resolution stays in the filesystem tool.
// ponytail: the underlying tool still loads the whole file; stream records if archive memory use becomes a problem.
pub(super) fn bounded_read_result(args: &Value, mut result: Value) -> Result<Value, ToolCallError> {
    let invalid = |message: &str| ToolCallError::RuntimeError(message.to_owned().into());
    let number = |key: &str, default| -> Result<usize, ToolCallError> {
        match args.get(key) {
            None => Ok(default),
            Some(value) => value
                .as_u64()
                .and_then(|v| v.try_into().ok())
                .ok_or_else(|| invalid("Read offsets/record numbers must be nonnegative integers")),
        }
    };
    let mut text = result["content"]
        .as_str()
        .ok_or_else(|| invalid("File content is not text"))?
        .to_owned();
    let offset = number("offset", 0)?;
    let decode_tool_payload = match args.get("decode_tool_payload") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| invalid("decode_tool_payload must be boolean"))?,
    };
    let needle = args
        .get("find")
        .map(|value| {
            value
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| invalid("find must be a nonempty string"))
        })
        .transpose()?;
    let pointer = match args.get("json_pointer") {
        None => "/content",
        Some(value) => value
            .as_str()
            .ok_or_else(|| invalid("json_pointer must be a string"))?,
    };
    if needle.is_some() {
        result["find"] = serde_json::to_value(needle)?;
        result["match_count"] = 0.into();
        result["last_match_offset"] = Value::Null;
    }
    let record = if args.get("jsonl_record").is_some() {
        Some(number("jsonl_record", 0)?)
    } else if let Some(needle) = needle.filter(|_| {
        result["path"]
            .as_str()
            .is_some_and(|p| p.ends_with(".jsonl"))
    }) {
        let records = text
            .lines()
            .map(serde_json::from_str::<Value>)
            .collect::<Result<Vec<_>, _>>()?;
        result["total_records"] = records.len().into();
        let original = |record: &Value| {
            (matches!(record["role"].as_str(), Some("User" | "Tool"))
                || record["evidence_kind"] == "original_exchange")
                && archive_field_kind(record, pointer) != "recovery"
                && record.pointer(pointer).is_some_and(Value::is_string)
        };
        let originals = records.iter().any(original);
        let recovery = records
            .iter()
            .any(|record| archive_field_kind(record, pointer) == "recovery");
        let matched = records.iter().enumerate().find(|(_, record)| {
            (original(record) || (!originals && !recovery))
                && decoded_archive_field(record, pointer, decode_tool_payload).is_some_and(
                    |field| {
                        let byte = field
                            .char_indices()
                            .nth(offset)
                            .map_or(field.len(), |(n, _)| n);
                        field[byte..].contains(needle)
                    },
                )
        });
        if let Some((index, _)) = matched {
            Some(index + 1)
        } else {
            result["content"] = "".into();
            result["found"] = false.into();
            result["next_offset"] = Value::Null;
            let scope = if originals || recovery {
                "Original User/Tool or original_exchange"
            } else {
                "All"
            };
            result["search_scope"] =
                format!("{scope} JSONL records at {pointer}, after character {offset}").into();
            return Ok(result);
        }
    } else {
        None
    };
    if let Some(record) = record {
        // Identify the complete authorized source before selecting or decoding a field.
        // This is provenance for a secondary excerpt, not proof that its content is true.
        use sha2::Digest;
        result["source_sha256"] = format!("{:x}", sha2::Sha256::digest(text.as_bytes())).into();
        result["total_records"] = text.lines().count().into();
        let line = record
            .checked_sub(1)
            .and_then(|n| text.lines().nth(n))
            .ok_or_else(|| {
                let available = text.lines().take(8).enumerate().map(|(index, line)| {
                    let decoded = serde_json::from_str::<Value>(line).unwrap_or(Value::Null);
                    let role = decoded["role"].as_str()
                        .filter(|role| matches!(*role, "User" | "Assistant" | "Tool" | "System"))
                        .unwrap_or("Unknown");
                    serde_json::json!({"record":index + 1,"role":role})
                }).collect::<Vec<_>>();
                invalid(&format!("JSONL record {record} does not exist (record numbers start at 1). Total records: {}. First up to 8 records: {}. Retry with an available record, or omit jsonl_record and use find to search decoded records; Assistant original_exchange records are historical exchanges; other Assistant records are navigation.", text.lines().count(), serde_json::json!(available)))
            })?;
        let decoded: Value = serde_json::from_str(line).map_err(|_| {
            if result["path"].as_str().is_some_and(|path| path.ends_with(".jsonl")) {
                invalid("Selected JSONL record is malformed; JSONL selectors require a valid JSON value per record. Verify the file format and record number; no source field was selected.")
            } else {
                invalid("JSONL selectors require JSONL content. For ordinary source/text files omit jsonl_record and json_pointer, and omit decode_tool_payload; read using file_path with optional offset/find. No source field was selected.")
            }
        })?;
        text = decoded_archive_field(&decoded, pointer, decode_tool_payload)
            .ok_or_else(|| invalid("Selected JSONL field is missing or is not a string"))?;
        result["jsonl_record"] = record.into();
        result["json_pointer"] = pointer.into();
        result["record_role"] = decoded["role"].clone();
        result["evidence_kind"] = archive_field_kind(&decoded, pointer).into();
        result["decode_tool_payload"] = decode_tool_payload.into();
        if archive_field_kind(&decoded, pointer) == "recovery" {
            result["evidence_warning"] = crate::prompts::TOOL_READ_RECOVERY_WARNING.into();
        }
        if decoded["evidence_kind"] == "original_exchange" {
            result["evidence_warning"] = crate::prompts::TOOL_READ_EXCHANGE_WARNING.into();
        } else if decoded["role"] == "Assistant" {
            result["evidence_warning"] = crate::prompts::TOOL_READ_NAVIGATION_WARNING.into();
            result["navigation_header"] = text
                .lines()
                .take(2)
                .collect::<Vec<_>>()
                .join("\n")
                .chars()
                .take(1000)
                .collect::<String>()
                .into();
        }
    } else if args.get("json_pointer").is_some() {
        return Err(invalid("json_pointer requires jsonl_record"));
    }
    let total = text.chars().count();
    if offset > total {
        return Err(invalid(&format!("Read offset {offset} is past the end: the text has {total} characters, so everything was already read")));
    }
    let mut start = offset;
    let mut next = None;
    let mut found = true;
    if let Some(needle) = needle {
        let byte = text
            .char_indices()
            .nth(offset)
            .map_or(text.len(), |(n, _)| n);
        let mut matches = text[byte..].match_indices(needle);
        if let Some((index, _)) = matches.next() {
            let (count, last) =
                matches.fold((1usize, index), |(count, _), (index, _)| (count + 1, index));
            let matched = offset + text[byte..byte + index].chars().count();
            result["match_offset"] = matched.into();
            result["match_count"] = count.into();
            result["last_match_offset"] = (offset + text[byte..byte + last].chars().count()).into();
            start = matched.saturating_sub(200);
            next = Some(matched + needle.chars().count());
        } else {
            found = false;
        }
    }
    let excerpt: String = if found {
        text.chars().skip(start).take(READ_PAGE_CHARS).collect()
    } else {
        String::new()
    };
    let end = start + excerpt.chars().count();
    if args.get("find").is_none() && end < total {
        next = Some(end);
    }
    result["content"] = excerpt.into();
    result["found"] = found.into();
    result["excerpt_offset"] = start.into();
    result["total_chars"] = total.into();
    result["next_offset"] = serde_json::to_value(next)?;
    result["truncated"] = (found && end < total).into();
    Ok(result)
}

/// One-line summary for filesystem toolkit calls.
///
/// Unknown tool names and malformed args fall back to a capped
/// ``call `<name>` with <args>`` rendering so the approval UI always shows
/// something meaningful.
pub fn summarize_fs_call(name: &str, args: &Value) -> String {
    fn field<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
        args.get(key).and_then(Value::as_str)
    }
    fn flag(args: &Value, key: &str) -> bool {
        args.get(key).and_then(Value::as_bool).unwrap_or(false)
    }
    let summary = match name {
        "write_file" => field(args, "file_path").map(|path| {
            if flag(args, "append") {
                format!("append to `{path}`")
            } else {
                format!("write `{path}`")
            }
        }),
        "read_file" => field(args, "file_path").map(|path| format!("read `{path}`")),
        "list_dir" => field(args, "directory_path").map(|path| format!("list `{path}`")),
        "search_file" => match (field(args, "directory"), field(args, "pattern")) {
            (Some(dir), Some(pattern)) => Some(format!("search `{pattern}` in `{dir}`")),
            (None, Some(pattern)) => Some(format!("search `{pattern}`")),
            _ => None,
        },
        "copy_file" => match (field(args, "source_path"), field(args, "destination_path")) {
            (Some(from), Some(to)) => Some(format!("copy `{from}` -> `{to}`")),
            _ => None,
        },
        "move_file" => match (field(args, "source_path"), field(args, "destination_path")) {
            (Some(from), Some(to)) => Some(format!("move `{from}` -> `{to}`")),
            _ => None,
        },
        "create_dir" => field(args, "directory_path").map(|path| format!("create dir `{path}`")),
        "delete_file" => field(args, "path").map(|path| {
            if flag(args, "recursive") {
                format!("delete `{path}` (recursive)")
            } else {
                format!("delete `{path}`")
            }
        }),
        _ => None,
    };
    if let Some(summary) = summary {
        return summary;
    }
    const MAX_CHARS: usize = 200;
    let single_line = args.to_string();
    let single_line = single_line.split_whitespace().collect::<Vec<_>>().join(" ");
    let full = format!("call `{name}` with {single_line}");
    if full.chars().count() <= MAX_CHARS {
        return full;
    }
    let mut truncated: String = full.chars().take(MAX_CHARS).collect();
    truncated.push('…');
    truncated
}
