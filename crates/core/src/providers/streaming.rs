//! Streamed chat answers: reading server-sent events into stream chunks and tool calls.

use super::*;

/// Which call a streamed tool-call piece belongs to: its `index`, else (some providers,
/// such as iceman-contributor-free on Go, stream each call whole with its own id and no
/// index) a new call for a new id and the last call for a piece without one. Keying them
/// all on 0 glued the calls into one unknown tool.
pub(super) fn streamed_call_index(
    calls: &std::collections::BTreeMap<usize, ToolCall>,
    call: &serde_json::Value,
) -> usize {
    if let Some(index) = call["index"].as_u64() {
        return usize::try_from(index).unwrap_or(usize::MAX);
    }
    let Some((&last, entry)) = calls.iter().next_back() else {
        return 0;
    };
    match call["id"].as_str().filter(|id| !id.is_empty()) {
        Some(id) if !entry.id.is_empty() && entry.id != id => last.saturating_add(1),
        _ => last,
    }
}

pub(super) struct ChatEventStream {
    response: reqwest::Response,
    buffer: Vec<u8>,
    ready: std::collections::VecDeque<Result<StreamChunk, LLMError>>,
    calls: std::collections::BTreeMap<usize, ToolCall>,
    reasoning: reasoning::StreamedReasoning,
    messages_api: bool,
    responses_api: bool,
    gemini_api: bool,
    done: bool,
    finished_choice: bool,
    pub(super) trace: Option<crate::context_metrics::RequestTrace>,
}

impl ChatEventStream {
    pub(super) fn new(response: reqwest::Response, protocol: Protocol) -> Self {
        Self {
            response,
            buffer: Vec::new(),
            ready: std::collections::VecDeque::new(),
            calls: std::collections::BTreeMap::new(),
            reasoning: reasoning::StreamedReasoning::default(),
            messages_api: protocol == Protocol::Messages,
            responses_api: protocol == Protocol::Responses,
            gemini_api: protocol == Protocol::Gemini,
            done: false,
            finished_choice: false,
            trace: None,
        }
    }

    pub(super) async fn next_chunk(&mut self) -> Option<Result<StreamChunk, LLMError>> {
        loop {
            if let Some(item) = self.ready.pop_front() {
                return Some(item);
            }
            if self.done {
                return None;
            }
            if let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let line = self.buffer.drain(..=end).collect();
                if let Err(error) = self.process_line(line) {
                    self.fail(error);
                }
                continue;
            }
            match self.response.chunk().await {
                Ok(Some(bytes)) => {
                    note_stream_progress();
                    self.buffer.extend_from_slice(&bytes);
                    if self.buffer.len() > 4 * 1024 * 1024 {
                        self.fail(LLMError::Generic(
                            "Provider stream event exceeded size limit".to_owned(),
                        ));
                    }
                }
                Ok(None) if self.finished_choice => {
                    self.done = true;
                    if let Some(trace) = &mut self.trace {
                        trace.complete();
                    }
                }
                Ok(None) => self.fail(LLMError::Generic(
                    "Provider stream ended before completion".to_owned(),
                )),
                Err(error) => self.fail(transport_error("stream interrupted", error)),
            }
        }
    }

    fn process_line(&mut self, line: Vec<u8>) -> Result<(), LLMError> {
        let line = std::str::from_utf8(&line)
            .map_err(|error| LLMError::Generic(format!("Invalid UTF-8 stream: {error}")))?
            .trim();
        let Some(data) = line.strip_prefix("data:").map(str::trim) else {
            return Ok(());
        };
        if data == "[DONE]" {
            if let Some(trace) = &mut self.trace {
                trace.complete();
            }
            self.done = true;
            return Ok(());
        }
        self.process_payload(data)
    }

    fn process_payload(&mut self, data: &str) -> Result<(), LLMError> {
        let value: serde_json::Value = serde_json::from_str(data)
            .map_err(|error| LLMError::Generic(format!("Invalid stream event: {error}")))?;
        if let Some(trace) = &mut self.trace {
            trace.usage(&value["usage"]);
            trace.usage(&value["message"]["usage"]);
            trace.usage(&value["response"]["usage"]);
        }
        if value["type"] == "message_stop" || value["type"] == "response.completed" {
            self.done = true;
            if let Some(trace) = &mut self.trace {
                trace.complete();
            }
        }
        if let Some(error) = value.get("error") {
            return Err(LLMError::Generic(format!("Provider stream error: {error}")));
        }
        if self.responses_api
            && value["type"] == "response.incomplete"
            && value["response"]["incomplete_details"]["reason"] == "max_output_tokens"
        {
            // The same cut-off as a non-streamed reply, so it gets the cut-off retries.
            return Err(LLMError::Generic(
                "Provider response was truncated".to_owned(),
            ));
        }
        if self.responses_api
            && matches!(
                value["type"].as_str(),
                Some("response.failed" | "response.incomplete")
            )
        {
            return Err(LLMError::Generic(
                "Provider response failed or was incomplete".to_owned(),
            ));
        }
        if self.responses_api && value["type"] == "response.function_call_arguments.done" {
            let index = value["output_index"].as_u64().unwrap_or(0) as usize;
            if let Some(call) = self.calls.get_mut(&index) {
                if let Some(args) = value["arguments"]
                    .as_str()
                    .or_else(|| value["item"]["arguments"].as_str())
                {
                    call.function.arguments = args.to_owned();
                }
            }
            return Ok(());
        }
        if self.gemini_api {
            if let Some(trace) = &mut self.trace {
                trace.usage(&gemini_usage(&value["usageMetadata"]));
            }
        }
        let value = if self.gemini_api {
            gemini_stream_event(value, self.calls.len())?
        } else if self.responses_api {
            responses_stream_event(value)
        } else if self.messages_api {
            messages_stream_event(value)
        } else {
            value
        };
        for choice in value["choices"].as_array().into_iter().flatten() {
            self.process_choice(choice)?;
        }
        Ok(())
    }

    fn process_choice(&mut self, choice: &serde_json::Value) -> Result<(), LLMError> {
        let delta = &choice["delta"];
        if let Some(text) = delta["content"].as_str().filter(|text| !text.is_empty()) {
            self.ready.push_back(Ok(StreamChunk::Text(text.to_owned())));
        }
        self.reasoning.take(delta);
        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            self.merge_tool_call(call);
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish_choice(reason)?;
        }
        Ok(())
    }

    fn merge_tool_call(&mut self, call: &serde_json::Value) {
        let index = streamed_call_index(&self.calls, call);
        let entry = self.calls.entry(index).or_insert_with(|| ToolCall {
            id: String::new(),
            call_type: "function".to_owned(),
            function: autoagents::llm::FunctionCall {
                name: String::new(),
                arguments: String::new(),
            },
        });
        if let Some(id) = call["id"].as_str() {
            // A provider that repeats the id on every piece of the same call keeps one copy.
            if entry.id != id {
                entry.id.push_str(id);
            }
        }
        if let Some(name) = call["function"]["name"].as_str() {
            entry.function.name.push_str(name);
        }
        if let Some(args) = call["function"]["arguments"].as_str() {
            entry.function.arguments.push_str(args);
        }
    }

    fn finish_choice(&mut self, reason: &str) -> Result<(), LLMError> {
        if reason == "length" {
            return Err(LLMError::Generic(
                "Provider response was truncated".to_owned(),
            ));
        }
        let calls: Vec<ToolCall> = self.calls.values().cloned().collect();
        self.reasoning.finish(&calls);
        for (index, mut tool_call) in std::mem::take(&mut self.calls) {
            if self.messages_api && tool_call.function.arguments.is_empty() {
                tool_call.function.arguments = "{}".to_owned();
            }
            self.ready
                .push_back(Ok(StreamChunk::ToolUseComplete { index, tool_call }));
        }
        self.ready.push_back(Ok(StreamChunk::Done {
            stop_reason: reason.to_owned(),
        }));
        self.finished_choice = true;
        Ok(())
    }

    fn fail(&mut self, error: LLMError) {
        self.ready.push_back(Err(error));
        self.done = true;
    }
}
