use super::*;

/// Most files one call may make a path guard look at: more is refused, not half checked.
const MAX_GUARDED_PATHS: usize = 64;

impl HookRuntime {
    /// A tool's name in Claude's words: its MCP name, its counterpart, or its own.
    pub(super) fn claude_name(&self, tool: &str) -> String {
        self.mcp_names
            .get(tool)
            .and_then(|names| names.first().cloned())
            .or_else(|| claude_tool_name(tool).map(str::to_owned))
            .unwrap_or_else(|| tool.to_owned())
    }

    /// Adds a line to the chat's transcript, in the shape Claude writes: one JSON object a line,
    /// with `"role":"…"` as hooks grep for it. Only the chat's own run writes there, and a failure
    /// only costs a line: it never stops the work.
    fn write_transcript(&self, role: &str, content: Value) {
        if self.agent.is_some() || self.hooks.iter().all(|hook| !hook.enabled) {
            return;
        }
        let Some(path) = &self.session.transcript_path else {
            return;
        };
        let line = json!({"type": role, "sessionId": self.thread_id,
            "message": {"role": role, "content": content}});
        let _ = append_transcript(path, &line.to_string());
    }

    /// Whether this run's hooks haven't heard of this skill yet.
    pub(super) fn first_sight_of(&self, skill: &str) -> bool {
        self.session
            .skills_seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(skill.to_owned())
    }

    pub(super) fn forget_skill(&self, skill: &str) {
        self.session
            .skills_seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(skill);
    }

    /// Whether the read that follows the first sight of a skill is still to be reported.
    pub(super) fn report_after(&self, skill: &str) -> bool {
        self.first_sight_of(&format!("after:{skill}"))
    }

    /// The hooks for skills the person attached: they go into the turn with no call, so this is
    /// when they are taken up, and when a `/skill` they wrote is expanded. A hook that refuses
    /// stops the turn, as a prompt hook does. What `UserPromptExpansion` hooks add is returned.
    pub(super) async fn attached_skill_hooks(
        &self,
        task: &str,
        events: &tokio::sync::mpsc::Sender<crate::runtime::RunEvent>,
    ) -> anyhow::Result<String> {
        let mut added = String::new();
        for skill in &self.session.attached_skills {
            if !self.first_sight_of(skill) {
                continue;
            }
            let expansion = json!({
                "expansion_type": "slash_command",
                "command_name": skill,
                "command_args": command_args(task, skill),
                "command_source": if skill.contains(':') { "plugin" } else { "user" },
                "prompt": task,
            });
            let outputs = self
                .emit_collect("UserPromptExpansion", expansion, events)
                .await
                .map_err(|error| anyhow::anyhow!("Skill {skill} was refused: {error}"))?;
            add_context(&mut added, &outputs);
            let call = json!({"tool": "Skill", "arguments": {"skill": skill}});
            self.emit("BeforeTool", call, events)
                .await
                .map_err(|error| anyhow::anyhow!("Skill {skill} was refused: {error}"))?;
            let done = json!({"tool": "Skill", "arguments": {"skill": skill}, "ok": true,
                "output": json!({"success": true, "commandName": skill}).to_string()});
            self.first_sight_of(&format!("after:{skill}"));
            self.emit("AfterTool", done, events).await.ok();
        }
        Ok(added)
    }

    /// This run's part in the chat's session: starts what hasn't started and asks about what will
    /// run with no run to ask. A `SessionStart` hook can't stop a turn, in Claude either. Returns
    /// what the session's hooks have said, for every turn of it.
    pub(super) async fn session_hooks(
        &self,
        events: &tokio::sync::mpsc::Sender<crate::runtime::RunEvent>,
    ) -> String {
        let Some(open) = self.session.open.clone() else {
            return String::new();
        };
        self.ask_about_unattended(&open);
        if open.opening {
            let mut seen = std::collections::HashSet::new();
            for file in self
                .hooks
                .iter()
                .filter(|hook| hook.enabled)
                .filter_map(|hook| hook.env_file.as_ref())
            {
                if seen.insert(&file.path) {
                    let _ = std::fs::remove_file(&file.path);
                }
            }
        }
        let mut said = read_session_context(&open.context_path);
        let mut changed = false;
        for hook in self
            .hooks
            .iter()
            .filter(|hook| hook.event == "SessionStart")
        {
            if !hook.enabled {
                continue;
            }
            let identity = hook_identity(hook);
            // A hook's first run in a chat is its startup, so one turned on in a chat that's already
            // going still starts; after that each session is a resume. One written only for
            // resuming runs when a chat with history opens a session, as that is one.
            let takes = |source: &str| {
                hook.moment_matcher
                    .as_ref()
                    .is_none_or(|pattern| moment_matches(pattern, source))
            };
            let source = if said.contains_key(&identity) || (open.resuming && !takes("startup")) {
                "resume"
            } else {
                "startup"
            };
            let wanted = takes(source);
            if !wanted
                || !open
                    .started
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(identity.clone())
            {
                continue;
            }
            let alone = HookRuntime {
                hooks: vec![hook.clone()],
                ..self.clone()
            };
            let payload = json!({"source": source, "model": self.session.model});
            let outputs = alone
                .emit_collect("SessionStart", payload, events)
                .await
                .unwrap_or_default();
            let mut text = String::new();
            add_context(&mut text, &outputs);
            said.insert(identity, capped(&text, PROMPT_CONTEXT_LIMIT).to_owned());
            changed = true;
        }
        if changed {
            let known: std::collections::HashSet<String> =
                self.hooks.iter().map(hook_identity).collect();
            said.retain(|identity, _| known.contains(identity));
            write_session_context(&open.context_path, &said);
        }
        self.env_refusals();
        for (source, path) in &open.config_changes {
            let payload = json!({"source": source, "file_path": path});
            self.emit("ConfigChange", payload, events).await.ok();
        }
        if open.opening {
            for path in &open.instructions {
                let payload = json!({"file_path": path, "memory_type": "Project",
                    "load_reason": "session_start"});
                self.emit("InstructionsLoaded", payload, events).await.ok();
            }
        }
        let mut context = String::new();
        for hook in self.hooks.iter().filter(|hook| hook.enabled) {
            if let Some(text) = said
                .get(&hook_identity(hook))
                .filter(|text| !text.is_empty())
            {
                if !context.is_empty() {
                    context.push('\n');
                }
                context.push_str(text);
            }
        }
        context
    }

    /// Asks once a session about each hook that will run when there's no run to ask.
    fn ask_about_unattended(&self, open: &OpenSession) {
        for hook in self.hooks.iter().filter(|hook| {
            hook.enabled && UNATTENDED_EVENTS.contains(&canonical_event(&hook.event))
        }) {
            let identity = hook_identity(hook);
            let known = open
                .unattended
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(&identity);
            if !known {
                let allowed = authorize(hook, &self.approvals).is_ok();
                if !allowed {
                    crate::diagnostics::emit(
                        "hook",
                        "unattended_not_allowed",
                        "warn",
                        json!({"hook": hook.name, "event": hook.event}),
                    );
                }
                open.unattended
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(identity, allowed);
            }
        }
    }

    /// Runs an event's hooks with no run around them, each within its own limit and all within
    /// `budget`. Only what was allowed when the session opened runs. It waits for them, whichever
    /// way they were written: nothing is left to report to, and a report to an ended session goes
    /// nowhere. Their answers are not read.
    pub async fn run_unattended(
        &self,
        event: &str,
        payload: Value,
        allowed: &std::collections::HashMap<String, bool>,
        budget: std::time::Duration,
    ) -> UnattendedOutcome {
        let payload = self.full_payload(event, payload);
        let mut outcome = UnattendedOutcome::default();
        let mut waiting = Vec::new();
        for hook in self.hooks.iter().filter(|hook| {
            hook.enabled
                && canonical_event(&hook.event) == canonical_event(event)
                && hook.applies(&payload, &self.mcp_names)
        }) {
            if allowed.get(&hook_identity(hook)) != Some(&true) {
                outcome.not_allowed += 1;
                continue;
            }
            let mut hook = hook.clone();
            hook.background = false;
            hook.rewake = None;
            let (root, payload) = (self.root.clone(), payload.clone());
            waiting.push(async move {
                IN_HOOK
                    .scope(true, execute(&hook, &root, &payload))
                    .await
                    .is_ok()
            });
        }
        outcome.ran = waiting.len();
        match tokio::time::timeout(budget, futures_util::future::join_all(waiting)).await {
            Ok(results) => outcome.failed = results.iter().filter(|ok| !**ok).count(),
            Err(_) => outcome.cut_short = true,
        }
        outcome
    }

    /// What the agent said, for the transcript.
    pub fn note_reply(&self, text: &str) {
        if !text.trim().is_empty() {
            self.write_transcript(
                "assistant",
                json!([{"type": "text", "text": entry_text(text)}]),
            );
        }
    }

    fn note_prompt(&self, text: &str) {
        self.write_transcript("user", json!(entry_text(text)));
    }

    /// A tool call or its result, for the transcript.
    fn note_tool(&self, event: &str, payload: &Value) {
        let name = payload["tool_name"].as_str().unwrap_or_default();
        match event {
            "BeforeTool" => {
                // An object, as Claude writes it: scripts read fields out of it. One past the bound
                // keeps what names the call.
                let input = &payload["tool_input"];
                let input = if input.to_string().len() <= TRANSCRIPT_ENTRY_BYTES {
                    input.clone()
                } else {
                    let field = |key: &str| input[key].as_str().map(|text| capped(text, 1024));
                    json!({"command": field("command"), "file_path": field("file_path"),
                        "truncated": true})
                };
                self.write_transcript(
                    "assistant",
                    json!([{"type": "tool_use", "name": name, "input": input}]),
                );
            }
            "AfterTool" => {
                let output = payload["output"]
                    .as_str()
                    .or_else(|| payload["error"].as_str())
                    .unwrap_or_default();
                self.write_transcript(
                    "user",
                    json!([{"type": "tool_result", "name": name, "content": entry_text(output)}]),
                );
            }
            _ => {}
        }
    }

    pub async fn emit(
        &self,
        event: &str,
        payload: Value,
        events: &tokio::sync::mpsc::Sender<crate::runtime::RunEvent>,
    ) -> anyhow::Result<()> {
        self.emit_collect(event, payload, events).await.map(|_| ())
    }

    /// A `prompt` hook: one request to the chat's model with no tools, within the hook's limit and
    /// the run's budget. The answer is hook output: a no blocks what a hook here can block, and an
    /// event past stopping has the agent told. Whatever else happens is a failure, which a guard
    /// holds on. Metadata only in the logs: never the question, the input or the answer.
    async fn ask_prompt_hook(
        &self,
        hook: &Hook,
        question: &str,
        payload: &Value,
    ) -> anyhow::Result<Value> {
        use autoagents::llm::chat::{ChatMessage, ChatRole, MessageType};
        authorize(hook, &self.approvals)?;
        let log = |outcome: &str, level: &str| {
            crate::diagnostics::emit(
                "hook",
                "prompt_hook",
                level,
                json!({"hook": hook.name, "event": hook.event, "outcome": outcome}),
            );
        };
        let Some(model) = &self.session.prompt_model else {
            log("no_model", "warn");
            bail!("No model for a prompt hook");
        };
        if model.off {
            log("off", "info");
            return Ok(json!({"ok": true}));
        }
        if !model.take_call() {
            log("budget_used_up", "warn");
            bail!("This run has used up its prompt hook requests");
        }
        // Only what the event is about: the rest of a hook's input is for scripts.
        let mut input = payload.clone();
        if let Some(fields) = input.as_object_mut() {
            for key in [
                "transcript_path",
                "project_root",
                "cwd",
                "run_id",
                "thread_id",
            ] {
                fields.remove(key);
            }
        }
        let input = input.to_string();
        let input = capped(&input, PROMPT_HOOK_INPUT_BYTES)
            .replace("<<<", "< < <")
            .replace(">>>", "> > >");
        let fenced = format!("<<<hook-input\n{input}\n>>>");
        let user = if question.contains("$ARGUMENTS") {
            question.replace("$ARGUMENTS", &fenced)
        } else {
            format!("{question}\n\n{fenced}")
        };
        let messages = [
            ChatMessage {
                role: ChatRole::System,
                message_type: MessageType::Text,
                content: crate::prompts::HOOK_PROMPT_SYSTEM.to_owned(),
            },
            ChatMessage {
                role: ChatRole::User,
                message_type: MessageType::Text,
                content: user,
            },
        ];
        let limit = std::time::Duration::from_secs(hook.timeout_seconds);
        let reply = match tokio::time::timeout(limit, model.llm.chat(&messages, None)).await {
            Ok(Ok(reply)) => reply.text().unwrap_or_default(),
            Ok(Err(_)) => {
                log("request_failed", "warn");
                bail!("A prompt hook's request failed");
            }
            Err(_) => {
                log("timed_out", "warn");
                bail!("Hook timed out");
            }
        };
        let Some(verdict) = parse_prompt_verdict(&reply) else {
            log("unreadable", "warn");
            bail!("A prompt hook's answer couldn't be read");
        };
        log(if verdict.ok { "ok" } else { "no" }, "info");
        if verdict.ok {
            return Ok(json!({"ok": true}));
        }
        let reason = verdict.reason;
        // What's past stopping can only be told: to the agent at its next step, labelled.
        if matches!(
            canonical_event(&hook.event),
            "AfterTool" | "SubagentEnd" | "PostToolBatch"
        ) && self.agent.is_none()
        {
            let output = reason.unwrap_or_else(|| "The plugin's check said no.".to_owned());
            leave_rewake_note(
                &self.thread_id,
                RewakeNote {
                    hook: hook.name.clone(),
                    message: None,
                    summary: first_line(&output),
                    output,
                },
            );
            return Ok(json!({"ok": false}));
        }
        Err(Blocked(reason).into())
    }

    /// Metadata only: how many names a plugin's session hooks wrote and were refused.
    fn env_refusals(&self) {
        let mut seen = std::collections::HashSet::new();
        for file in self
            .hooks
            .iter()
            .filter(|hook| hook.enabled)
            .filter_map(|hook| hook.env_file.as_ref())
        {
            if seen.insert(&file.path) {
                let refused = file.read().1.len();
                if refused > 0 {
                    crate::diagnostics::emit(
                        "hook",
                        "env_refused",
                        "warn",
                        json!({"count": refused}),
                    );
                }
            }
        }
    }

    /// Runs the `PromptSubmit` hooks for `prompt`. A hook that asks to block stops the turn;
    /// the context they add is returned, capped at [`PROMPT_CONTEXT_LIMIT`].
    pub async fn prompt_submit(
        &self,
        prompt: &str,
        events: &tokio::sync::mpsc::Sender<crate::runtime::RunEvent>,
    ) -> anyhow::Result<String> {
        self.note_prompt(prompt);
        let outputs = self
            .emit_collect("PromptSubmit", json!({"prompt": prompt}), events)
            .await?;
        let mut context = String::new();
        add_context(&mut context, &outputs);
        Ok(capped(&context, PROMPT_CONTEXT_LIMIT).to_owned())
    }

    /// `payload` with what every hook of `event` gets.
    fn full_payload(&self, event: &str, payload: Value) -> Value {
        let mut payload = payload;
        payload["event"] = canonical_event(event).into();
        payload["run_id"] = self.run_id.clone().into();
        payload["thread_id"] = self.thread_id.clone().into();
        payload["project_root"] = self.root.to_string_lossy().as_ref().into();
        if let Some(agent) = &self.agent {
            payload["agent"] = agent.clone();
        }
        add_claude_fields(&mut payload, self);
        payload
    }

    /// Whether an enabled hook waits for `event`, to save building what it would be told.
    pub(super) fn listens(&self, event: &str) -> bool {
        self.hooks
            .iter()
            .any(|hook| hook.enabled && canonical_event(&hook.event) == event)
    }

    /// Runs the matching hooks; returns the output of each one that succeeded.
    pub(super) async fn emit_collect(
        &self,
        event: &str,
        payload: Value,
        events: &tokio::sync::mpsc::Sender<crate::runtime::RunEvent>,
    ) -> anyhow::Result<Vec<Value>> {
        let mut outputs = Vec::new();
        let event = canonical_event(event);
        let payload = self.full_payload(event, payload);
        self.note_tool(event, &payload);
        // A guard written for Claude looks at one `file_path`: it sees each file a call touches
        // in turn. A native hook gets the arguments, and reads them as it likes.
        let touched = payload["tool"]
            .as_str()
            .filter(|_| matches!(event, "BeforeTool" | "AfterTool"))
            .map_or_else(Vec::new, |tool| paths_touched(tool, &payload["arguments"]));
        let applying = || {
            self.hooks.iter().filter(|h| {
                h.enabled
                    && canonical_event(&h.event) == event
                    && h.applies(&payload, &self.mcp_names)
            })
        };
        if touched.len() > MAX_GUARDED_PATHS
            && event == "BeforeTool"
            && applying().any(|hook| hook.exit_two_blocks && hook.blocking)
        {
            let hook = applying()
                .find(|hook| hook.blocking)
                .map_or("", |h| h.name.as_str());
            bail!(blocked_tool_note(
                hook,
                Some("This call touches too many files for the plugin's guard to check. Split it.")
            ));
        }
        let mut views: Vec<(&Hook, Value)> = Vec::new();
        for hook in applying() {
            if hook.exit_two_blocks && touched.len() > 1 {
                for path in touched.iter().take(MAX_GUARDED_PATHS) {
                    let mut view = payload.clone();
                    view["tool_input"]["file_path"] = path.as_str().into();
                    views.push((hook, view));
                }
            } else {
                views.push((hook, payload.clone()));
            }
        }
        for (hook, payload) in &views {
            let hook = *hook;
            if hook.background {
                self.start_in_background(hook, payload);
                continue;
            }
            // Dropping the running hook ends its process group.
            let running = async {
                match &hook.prompt {
                    Some(question) => self.ask_prompt_hook(hook, question, payload).await,
                    None => run(hook, &self.root, payload, &self.approvals).await,
                }
            };
            let outcome = tokio::select! {
                outcome = IN_HOOK.scope(true, running) => outcome,
                () = stopped(self.session.cancel.clone()) => Err(anyhow::anyhow!("Hook stopped")),
            };
            events
                .send(crate::runtime::RunEvent::ToolCallFinished {
                    tool: format!(
                        "hook_{}",
                        hook.runtime_identity.as_ref().unwrap_or(&hook.name)
                    ),
                    ok: outcome.is_ok(),
                    output: match &outcome {
                        Ok(v) => v.to_string(),
                        Err(e) => e.to_string(),
                    },
                })
                .await
                .ok();
            if event == "Stop" {
                // Asking to go on is a decision; failing says nothing, whatever the policy.
                match outcome {
                    Ok(value) => outputs.push(value),
                    Err(error) => {
                        if let Some(Blocked(reason)) = error.downcast_ref::<Blocked>() {
                            return Err(KeepGoing {
                                hook: hook.name.clone(),
                                reason: reason.clone(),
                            }
                            .into());
                        }
                    }
                }
                continue;
            }
            let stoppable = matches!(
                event,
                "PromptSubmit"
                    | "RunStart"
                    | "BeforeTool"
                    | "BeforeCompaction"
                    | "UserPromptExpansion"
                    | "PostToolBatch"
            );
            let abort = hook.failure_policy == Some(HookFailurePolicy::Abort)
                || (hook.failure_policy.is_none() && hook.blocking);
            match outcome {
                Ok(value) => outputs.push(value),
                // Asking to block is a decision, not a failure: the failure policy has no say.
                Err(error) if stoppable => {
                    if let Some(Blocked(reason)) = error.downcast_ref::<Blocked>() {
                        let reason = reason.as_deref();
                        if event == "BeforeTool" {
                            bail!(blocked_tool_note(&hook.name, reason));
                        }
                        bail!(blocked_message(&hook.name, reason));
                    }
                    if abort {
                        if event == "PromptSubmit" {
                            bail!("Blocked by hook {}: {error}", hook.name);
                        }
                        return Err(error);
                    }
                }
                Err(_) => {}
            }
        }
        Ok(outputs)
    }
}
