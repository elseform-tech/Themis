//! The agent's subagent tools (`spawn_subagents`, `wait_subagents`, `steer_subagent`,
//! `stop_subagent`), and the `status` field every subagent tool call accepts.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use themis_core::tools::{async_trait, ToolCallError, ToolRuntime, ToolT};

use super::subagents::{Reporter, Subagent, SubagentKit, MAX_PER_CALL};

/// Longest wait one `wait_subagents` call may ask for.
const MAX_WAIT_SECONDS: u64 = 600;
/// The longest brief, in characters.
const MAX_BRIEF: usize = 8000;

fn error(message: impl ToString) -> ToolCallError {
    ToolCallError::RuntimeError(message.to_string().into())
}

/// The subagent tools of one run, sharing its kit.
pub(super) fn tools(kit: &Arc<SubagentKit>) -> Vec<Box<dyn ToolT>> {
    [Kind::Spawn, Kind::Wait, Kind::Steer, Kind::Stop]
        .into_iter()
        .map(|kind| {
            Box::new(SubagentTool {
                kind,
                kit: Arc::clone(kit),
            }) as Box<dyn ToolT>
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Spawn,
    Wait,
    Steer,
    Stop,
}

struct SubagentTool {
    kind: Kind,
    kit: Arc<SubagentKit>,
}

impl std::fmt::Debug for SubagentTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubagentTool")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// The briefs the model asked for, named "Subagent N" when it gave no name.
fn parse_briefs(args: &Value) -> Result<Vec<(String, String)>, ToolCallError> {
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
            Ok((name, brief.chars().take(MAX_BRIEF).collect()))
        })
        .collect()
}

impl SubagentTool {
    fn agent(&self, args: &Value) -> Result<Arc<Subagent>, ToolCallError> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| error("needs `id`: the subagent's id from spawn_subagents"))?;
        self.kit
            .find(id)
            .ok_or_else(|| error(format!("no subagent with id '{id}' in this run")))
    }

    fn spawn(&self, args: &Value) -> Result<Value, ToolCallError> {
        let started: Vec<Value> = parse_briefs(args)?
            .into_iter()
            .map(|(name, brief)| {
                let agent = self.kit.start(name, brief);
                json!({"id": agent.id, "name": agent.name})
            })
            .collect();
        Ok(json!({"started": started, "running_at_once": self.kit.config.subagents.max_parallel}))
    }

    async fn wait(&self, args: &Value) -> Result<Value, ToolCallError> {
        let wanted: Vec<Arc<Subagent>> = match args["ids"].as_array() {
            Some(ids) => ids
                .iter()
                .filter_map(Value::as_str)
                .map(|id| {
                    self.kit
                        .find(id)
                        .ok_or_else(|| error(format!("no subagent with id '{id}' in this run")))
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
        Ok(json!({"subagents": subagents}))
    }

    fn steer(&self, args: &Value) -> Result<Value, ToolCallError> {
        let agent = self.agent(args)?;
        let message = args["message"]
            .as_str()
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .ok_or_else(|| error("steer_subagent needs `message`"))?;
        if agent.ended() {
            return Err(error(format!("subagent '{}' has already ended", agent.id)));
        }
        agent
            .inbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(message.chars().take(MAX_BRIEF).collect());
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
        match self.kind {
            Kind::Spawn => self.spawn(&args),
            Kind::Wait => self.wait(&args).await,
            Kind::Steer => self.steer(&args),
            Kind::Stop => self.stop(&args),
        }
    }
}

impl ToolT for SubagentTool {
    fn name(&self) -> &str {
        match self.kind {
            Kind::Spawn => "spawn_subagents",
            Kind::Wait => "wait_subagents",
            Kind::Steer => "steer_subagent",
            Kind::Stop => "stop_subagent",
        }
    }
    fn description(&self) -> &str {
        match self.kind {
            Kind::Spawn => themis_core::prompts::SPAWN_SUBAGENTS_TOOL,
            Kind::Wait => themis_core::prompts::WAIT_SUBAGENTS_TOOL,
            Kind::Steer => themis_core::prompts::STEER_SUBAGENT_TOOL,
            Kind::Stop => themis_core::prompts::STOP_SUBAGENT_TOOL,
        }
    }
    fn args_schema(&self) -> Value {
        match self.kind {
            Kind::Spawn => json!({"type":"object","properties":{
                "subagents":{"type":"array","minItems":1,"maxItems":MAX_PER_CALL,"items":{"type":"object","properties":{
                    "name":{"type":"string","description":"A short name, for example \"Bug hunt\""},
                    "brief":{"type":"string"}
                },"required":["brief"]}}
            },"required":["subagents"],"additionalProperties":false}),
            Kind::Wait => json!({"type":"object","properties":{
                "ids":{"type":"array","items":{"type":"string"}},
                "any":{"type":"boolean"},
                "timeout_seconds":{"type":"integer","minimum":1,"maximum":MAX_WAIT_SECONDS}
            },"additionalProperties":false}),
            Kind::Steer => json!({"type":"object","properties":{
                "id":{"type":"string"},"message":{"type":"string"}
            },"required":["id","message"],"additionalProperties":false}),
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
        assert_eq!(briefs[0], ("Bug hunt".into(), "Find bugs in src/".into()));
        assert_eq!(briefs[1], ("Subagent 2".into(), "Check docs".into()));
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
    fn the_status_field_joins_every_object_schema() {
        let schema = with_status(
            json!({"type": "object", "properties": {"path": {"type": "string"}},
            "required": ["path"], "additionalProperties": false}),
        );
        assert_eq!(schema["properties"]["status"]["maxLength"], 80);
        assert_eq!(schema["required"], json!(["path"]));
    }
}
