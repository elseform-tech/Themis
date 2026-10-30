//! The agent's subagent tools (`spawn_subagents`, `wait_subagents`, `message_subagent`,
//! `stop_subagent`), and the `status` field every subagent tool call accepts.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use themis_core::tools::{async_trait, ToolCallError, ToolRuntime, ToolT};

use super::subagents::{Reporter, Subagent, SubagentKit, MAX_PER_CALL};

/// The line the person sees before the call runs; the runtime shows it.
fn say_field() -> Value {
    json!({"type":"string","description":"At least two or three natural sentences for the person watching: what you are doing and why, in your own words, not repeating earlier ones."})
}

/// Longest wait one `wait_subagents` call may ask for.
const MAX_WAIT_SECONDS: u64 = 600;
/// The longest brief, in characters.
const MAX_BRIEF: usize = 8000;

fn error(message: impl ToString) -> ToolCallError {
    ToolCallError::RuntimeError(message.to_string().into())
}

/// The subagent tools of one run, sharing its kit.
pub(super) fn tools(kit: &Arc<SubagentKit>) -> Vec<Box<dyn ToolT>> {
    let others = kit.other_models();
    [Kind::Spawn, Kind::Wait, Kind::Message, Kind::Stop]
        .into_iter()
        .map(|kind| {
            let description = match kind {
                Kind::Spawn if !others.is_empty() => format!(
                    "{}\n\n{}",
                    themis_core::prompts::SPAWN_SUBAGENTS_TOOL,
                    themis_core::prompts::fill(
                        themis_core::prompts::SPAWN_SUBAGENTS_MODELS,
                        &[
                            ("models", &others.join(", ")),
                            ("default", &kit.default_model()),
                        ],
                    )
                ),
                Kind::Spawn => themis_core::prompts::SPAWN_SUBAGENTS_TOOL.to_owned(),
                Kind::Wait => themis_core::prompts::WAIT_SUBAGENTS_TOOL.to_owned(),
                Kind::Message => themis_core::prompts::MESSAGE_SUBAGENT_TOOL.to_owned(),
                Kind::Stop => themis_core::prompts::STOP_SUBAGENT_TOOL.to_owned(),
            };
            Box::new(SubagentTool {
                kind,
                kit: Arc::clone(kit),
                description,
                models: !others.is_empty(),
            }) as Box<dyn ToolT>
        })
        .collect()
}

/// The model a subagent runs on: the default without one, else the one allowed model the
/// request names, matched loosely ("haiku" finds `claude-haiku-4`; case, spaces and
/// dashes do not matter). Anything else is refused with the choices.
pub(super) fn pick_model(
    requested: Option<&str>,
    default: &str,
    others: &[String],
) -> Result<String, String> {
    let Some(requested) = requested.map(str::trim).filter(|model| !model.is_empty()) else {
        return Ok(default.to_owned());
    };
    let plain = |text: &str| {
        text.chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let wanted = plain(requested);
    let all: Vec<String> = std::iter::once(default.to_owned())
        .chain(others.iter().cloned())
        .collect();
    if let Some(exact) = all.iter().find(|model| plain(model) == wanted) {
        return Ok(exact.clone());
    }
    let close: Vec<&String> = all
        .iter()
        .filter(|model| !wanted.is_empty() && plain(model).contains(&wanted))
        .collect();
    match close.as_slice() {
        [one] => Ok((*one).clone()),
        _ => Err(format!(
            "subagent model '{requested}' is not available{}: use one of {}, or leave model out for {default}",
            if close.is_empty() { "" } else { " (it matches more than one)" },
            all.join(", ")
        )),
    }
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Spawn,
    Wait,
    Message,
    Stop,
}

struct SubagentTool {
    kind: Kind,
    kit: Arc<SubagentKit>,
    description: String,
    /// The spawn tool offers `model` (`subagents.models` lists some).
    models: bool,
}

impl std::fmt::Debug for SubagentTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubagentTool")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// One subagent the model asked for.
#[derive(Debug, PartialEq)]
struct Brief {
    name: String,
    brief: String,
    model: Option<String>,
    write_paths: Vec<String>,
}

/// The briefs the model asked for, named "Subagent N" when it gave no name.
fn parse_briefs(args: &Value) -> Result<Vec<Brief>, ToolCallError> {
    let list = args["subagents"]
        .as_array()
        .filter(|list| !list.is_empty())
        .ok_or_else(|| error("spawn_subagents needs `subagents`: a list of {name, brief}"))?;
    if list.len() > MAX_PER_CALL {
        return Err(error(format!(
            "spawn_subagents starts at most {MAX_PER_CALL} subagents per call"
        )));
    }
    list.iter()
        .enumerate()
        .map(|(index, item)| {
            let brief = item["brief"]
                .as_str()
                .map(str::trim)
                .filter(|brief| !brief.is_empty())
                .ok_or_else(|| error(format!("subagent {} needs a `brief`", index + 1)))?;
            let name = item["name"]
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(|name| name.chars().take(40).collect())
                .unwrap_or_else(|| format!("Subagent {}", index + 1));
            Ok(Brief {
                name,
                brief: brief.chars().take(MAX_BRIEF).collect(),
                model: item["model"].as_str().map(str::to_owned),
                write_paths: match &item["write_paths"] {
                    Value::Null => Vec::new(),
                    paths => paths
                        .as_array()
                        .and_then(|paths| {
                            paths
                                .iter()
                                .map(|path| path.as_str().map(str::to_owned))
                                .collect::<Option<Vec<_>>>()
                        })
                        .ok_or_else(|| {
                            error(format!(
                                "subagent {}: `write_paths` must be a list of paths",
                                index + 1
                            ))
                        })?,
                },
            })
        })
        .collect()
}

impl SubagentTool {
    fn agent(&self, args: &Value) -> Result<Arc<Subagent>, ToolCallError> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| error("needs `id`: the subagent's id from spawn_subagents"))?;
        self.kit
            .find_in_chat(id)
            .ok_or_else(|| error(format!("no subagent with id '{id}' in this chat")))
    }

    fn spawn(&self, args: &Value) -> Result<Value, ToolCallError> {
        let default = self.kit.default_model();
        let others = self.kit.other_models();
        // Every model is checked before any subagent starts.
        let briefs = parse_briefs(args)?
            .into_iter()
            .map(|brief| {
                pick_model(brief.model.as_deref(), &default, &others)
                    .map(|model| (brief, model))
                    .map_err(error)
            })
            .collect::<Result<Vec<_>, _>>()?;
        // `subagents.allow_writes` off keeps every subagent read-only, and the agent is told.
        let allow_writes = self.kit.config.subagents.allow_writes;
        let asked: Vec<Vec<String>> = briefs
            .iter()
            .map(|(brief, _)| {
                if allow_writes {
                    brief.write_paths.clone()
                } else {
                    Vec::new()
                }
            })
            .collect();
        let refused_writes = !allow_writes
            && briefs
                .iter()
                .any(|(brief, _)| !brief.write_paths.is_empty());
        // Every path is checked before any subagent starts.
        let write_paths = self.kit.check_write_paths(&asked).map_err(error)?;
        let started: Vec<Value> = briefs
            .into_iter()
            .zip(write_paths)
            .map(|((brief, model), paths)| {
                let agent = self.kit.start(brief.name, brief.brief, model, paths);
                json!({"id": agent.id, "name": agent.name, "model": agent.model,
                    "write_paths": agent.write_paths})
            })
            .collect();
        let mut output =
            json!({"started": started, "running_at_once": self.kit.config.subagents.max_parallel});
        if refused_writes {
            output["read_only"] =
                json!("Subagents are set to read-only here: write_paths were ignored.");
        }
        Ok(output)
    }

    async fn wait(&self, args: &Value) -> Result<Value, ToolCallError> {
        let wanted: Vec<Arc<Subagent>> = match args["ids"].as_array() {
            Some(ids) => ids
                .iter()
                .filter_map(Value::as_str)
                .map(|id| {
                    self.kit
                        .find_in_chat(id)
                        .ok_or_else(|| error(format!("no subagent with id '{id}' in this chat")))
                })
                .collect::<Result<_, _>>()?,
            None => self
                .kit
                .started
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        };
        let any = args["any"].as_bool().unwrap_or(false);
        let timeout = Duration::from_secs(
            args["timeout_seconds"]
                .as_u64()
                .unwrap_or(300)
                .clamp(1, MAX_WAIT_SECONDS),
        );
        let started = Instant::now();
        loop {
            let ended = wanted.iter().filter(|agent| agent.ended()).count();
            let done = if any {
                ended > 0
            } else {
                ended == wanted.len()
            };
            if done
                || started.elapsed() >= timeout
                || self.kit.stopped.load(std::sync::atomic::Ordering::SeqCst)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let subagents: Vec<Value> = wanted
            .iter()
            .map(|agent| {
                let progress = agent.progress();
                match progress.ended {
                    // Taken back from the notes, so it is not repeated before the next step.
                    Some((status, result)) if self.kit.notes.withdraw(&progress.note) => {
                        json!({"id": agent.id, "name": agent.name, "status": status, "result": result})
                    }
                    Some((status, _)) => {
                        json!({"id": agent.id, "name": agent.name, "status": status, "result_delivered_earlier": true})
                    }
                    None => json!({"id": agent.id, "name": agent.name, "status": "running"}),
                }
            })
            .collect();
        let reported = subagents.iter().any(|agent| agent.get("result").is_some());
        let mut output = json!({"subagents": subagents});
        if reported {
            output["check"] = themis_core::prompts::SUBAGENT_REVIEW.into();
        }
        Ok(output)
    }

    fn message(&self, args: &Value) -> Result<Value, ToolCallError> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| error("needs `id`: the subagent's id from spawn_subagents"))?;
        let agent = self
            .kit
            .find_in_chat(id)
            .ok_or_else(|| error(format!("no subagent with id '{id}' in this chat")))?;
        let message: String = args["message"]
            .as_str()
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .ok_or_else(|| error("message_subagent needs `message`"))?
            .chars()
            .take(MAX_BRIEF)
            .collect();
        if agent.ended() {
            self.kit.resume(&agent, message).map_err(error)?;
            return Ok(json!({"sent": true, "resumed": true,
                "note": "It is working again and will report back as before."}));
        }
        agent
            .inbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(message.clone());
        self.kit.show_message(&agent, &message, false);
        Ok(json!({"sent": true}))
    }

    fn stop(&self, args: &Value) -> Result<Value, ToolCallError> {
        let agent = self.agent(args)?;
        if agent.ended() {
            return Ok(json!({"stopping": false, "status": "already ended"}));
        }
        self.kit.stop(&agent);
        Ok(json!({"stopping": true}))
    }
}

#[async_trait]
impl ToolRuntime for SubagentTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        // No prompt (the dispatcher leaves these tools to approve themselves), but a deny
        // rule someone wrote still holds, in YOLO too.
        let action = themis_core::tools::ToolAction {
            display: None,
            command: Vec::new(),
            tool: self.name().to_owned(),
            summary: self.name().to_owned(),
            risk: themis_core::tools::RiskLevel::Read,
        };
        let policy = &self.kit.config.approval;
        let decision = if self.kit.mode == themis_core::configuration::ApprovalMode::Yolo {
            policy.yolo_decision(&action)
        } else {
            policy.decision(&action)
        };
        if decision == themis_core::configuration::PolicyAction::Deny {
            return Err(error(format!(
                "approval denied for '{}': the approval rules deny it",
                self.name()
            )));
        }
        // The run's `say` line travels through its event channel, but the subagent card goes
        // straight to the chat: give the line a moment to get there first.
        if matches!(self.kind, Kind::Spawn) {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        match self.kind {
            Kind::Spawn => self.spawn(&args),
            Kind::Wait => self.wait(&args).await,
            Kind::Message => self.message(&args),
            Kind::Stop => self.stop(&args),
        }
    }
}

impl ToolT for SubagentTool {
    fn name(&self) -> &str {
        match self.kind {
            Kind::Spawn => "spawn_subagents",
            Kind::Wait => "wait_subagents",
            Kind::Message => "message_subagent",
            Kind::Stop => "stop_subagent",
        }
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn args_schema(&self) -> Value {
        match self.kind {
            Kind::Spawn => {
                let mut item = json!({"type":"object","properties":{
                    "name":{"type":"string","description":"A short name, for example \"Bug hunt\""},
                    "brief":{"type":"string"},
                    "write_paths":{"type":"array","items":{"type":"string"},"description":"Project-relative folders (ending in /) or files it alone may change. Leave out for read-only."}
                },"required":["brief"]});
                if self.models {
                    item["properties"]["model"] = json!({"type":"string"});
                }
                json!({"type":"object","properties":{
                    "say":say_field(),
                    "subagents":{"type":"array","minItems":1,"maxItems":MAX_PER_CALL,"items":item}
                },"required":["say","subagents"],"additionalProperties":false})
            }
            Kind::Wait => json!({"type":"object","properties":{
                "ids":{"type":"array","items":{"type":"string"}},
                "any":{"type":"boolean"},
                "timeout_seconds":{"type":"integer","minimum":1,"maximum":MAX_WAIT_SECONDS},
                "say":say_field()
            },"required":["say"],"additionalProperties":false}),
            Kind::Message => json!({"type":"object","properties":{
                "id":{"type":"string"},"message":{"type":"string"},"say":say_field()
            },"required":["say","id","message"],"additionalProperties":false}),
            Kind::Stop => json!({"type":"object","properties":{
                "id":{"type":"string"}
            },"required":["id"],"additionalProperties":false}),
        }
    }
    fn output_schema(&self) -> Option<Value> {
        None
    }
}

/// A subagent's tool with the optional `status` line: taken out of the arguments before
/// the tool runs and shown to the person watching.
pub(super) struct StatusField {
    pub(super) inner: Box<dyn ToolT>,
    pub(super) agent: Arc<Subagent>,
    pub(super) report: Reporter,
}

impl std::fmt::Debug for StatusField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatusField")
            .field("tool", &self.inner.name())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ToolRuntime for StatusField {
    async fn execute(&self, mut args: Value) -> Result<Value, ToolCallError> {
        let status = args.as_object_mut().and_then(|args| args.remove("status"));
        match status.as_ref().and_then(Value::as_str) {
            Some(status) => self.agent.set_status(status),
            None => self.agent.set_action(&action(self.inner.name(), &args)),
        }
        (self.report)(&self.agent);
        self.inner.execute(args).await
    }
}

impl ToolT for StatusField {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn description(&self) -> &str {
        self.inner.description()
    }
    fn args_schema(&self) -> Value {
        with_status(self.inner.args_schema())
    }
    fn output_schema(&self) -> Option<Value> {
        self.inner.output_schema()
    }
}

/// A short readable form of a call, for the status line.
fn action(tool: &str, args: &Value) -> String {
    if let Some(command) = themis_core::tools::command_display_for_call(tool, args) {
        return format!("Run `{}`", command.argv.join(" "));
    }
    let summary = themis_core::tools::summarize_fs_call(tool, args);
    let mut chars = summary.chars();
    let first = chars.next().map(|c| c.to_uppercase().to_string());
    first.unwrap_or_default() + chars.as_str()
}

fn with_status(mut schema: Value) -> Value {
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        properties.insert("status".into(), json!({"type": "string", "maxLength": 80}));
    }
    schema
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn briefs_are_read_leniently_and_bounded() {
        let briefs = parse_briefs(&json!({"subagents": [
            {"name": "Bug hunt", "brief": "Find bugs in src/"},
            {"brief": "  Check docs  "}
        ]}))
        .unwrap();
        assert_eq!(
            (briefs[0].name.as_str(), briefs[0].brief.as_str()),
            ("Bug hunt", "Find bugs in src/")
        );
        assert_eq!(
            (briefs[1].name.as_str(), briefs[1].brief.as_str()),
            ("Subagent 2", "Check docs")
        );
        assert!(parse_briefs(&json!({"subagents": []})).is_err());
        assert!(parse_briefs(&json!({"subagents": [{"name": "x"}]})).is_err());
        let seven = vec![json!({"brief": "b"}); MAX_PER_CALL + 1];
        assert!(parse_briefs(&json!({ "subagents": seven })).is_err());
    }

    #[test]
    fn actions_read_as_short_lines() {
        assert_eq!(
            action("read_file", &json!({"file_path": "src/stats.py"})),
            "Read `src/stats.py`"
        );
        assert_eq!(
            action("shell", &json!({"command": "cargo", "args": ["test"]})),
            "Run `cargo test`"
        );
    }

    #[test]
    fn a_named_model_is_matched_to_the_allowed_ones_or_refused() {
        let others = vec!["glm-5".to_owned(), "claude-haiku-4".to_owned()];
        let pick = |wanted: Option<&str>| pick_model(wanted, "beam-free", &others);
        assert_eq!(pick(None).unwrap(), "beam-free");
        assert_eq!(pick(Some("GLM 5")).unwrap(), "glm-5");
        assert_eq!(pick(Some("haiku")).unwrap(), "claude-haiku-4");
        assert_eq!(pick(Some("Beam-Free")).unwrap(), "beam-free");
        let refused = pick(Some("claude-opus")).unwrap_err();
        assert!(
            refused.contains("beam-free, glm-5, claude-haiku-4"),
            "{refused}"
        );
        // A word in several names is not a choice.
        let both = pick_model(Some("glm"), "beam-free", &["glm-5".into(), "glm-4".into()]);
        assert!(both.unwrap_err().contains("more than one"));
    }

    #[test]
    fn the_status_field_joins_every_object_schema() {
        let schema = with_status(
            json!({"type": "object", "properties": {"path": {"type": "string"}},
            "required": ["path"], "additionalProperties": false}),
        );
        assert_eq!(schema["properties"]["status"]["maxLength"], 80);
        assert_eq!(schema["required"], json!(["path"]));
    }
}
