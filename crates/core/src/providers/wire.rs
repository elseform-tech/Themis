use super::*;

/// OpenAI chat-completions message (owned subset sufficient for text + tools).
#[derive(Serialize)]
pub(super) struct WireMessage {
    pub(super) role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_call_id: Option<String>,
    /// A thinking model's reasoning for these tool calls, when replay is on.
    #[serde(flatten)]
    pub(super) reasoning: super::reasoning::WireReasoning,
    /// An image after `content` (a user message); each protocol writes it its own way.
    #[serde(skip)]
    pub(super) image: Option<WireImage>,
}

/// An image for a model that reads images: its media type and base64 bytes.
pub(super) struct WireImage {
    pub(super) mime: &'static str,
    pub(super) data: String,
}

impl WireImage {
    /// The `data:` URL the OpenAI shapes take.
    pub(super) fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime, self.data)
    }
}

/// `true` when `messages` hold an image (only models that read images may get them).
pub(super) fn has_images(messages: &[ChatMessage]) -> bool {
    messages
        .iter()
        .any(|message| matches!(message.message_type, MessageType::Image(_)))
}

/// One Chat Completions message as JSON. A message with an image carries content parts:
/// its text, then the image as a `data:` URL. Tool-role messages cannot carry images in the
/// OpenAI-compatible APIs, so the runtime puts each image in a user message right after the
/// tool results.
pub(super) fn chat_message_value(message: WireMessage) -> Result<serde_json::Value, LLMError> {
    let image = message.image.as_ref().map(WireImage::data_url);
    let text = message.content.clone().unwrap_or_default();
    let mut value = serde_json::to_value(message)?;
    if let Some(url) = image {
        value["content"] = serde_json::json!([
            {"type": "text", "text": text},
            {"type": "image_url", "image_url": {"url": url}},
        ]);
    }
    Ok(value)
}

/// OpenAI chat-completions request body.
#[derive(Serialize)]
pub(super) struct WireChatRequest<'a> {
    pub(super) model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reasoning_effort: Option<&'a str>,
    pub(super) messages: Vec<serde_json::Value>,
    pub(super) stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tools: Option<&'a [Tool]>,
}

/// OpenAI chat-completions response body (subset).
#[derive(Deserialize)]
pub(super) struct WireChatResponse {
    #[serde(default)]
    pub(super) choices: Vec<WireChoice>,
    #[serde(default)]
    pub(super) usage: Option<Usage>,
}

/// Single chat choice of [`WireChatResponse`].
#[derive(Deserialize)]
pub(super) struct WireChoice {
    #[serde(default)]
    pub(super) finish_reason: Option<String>,
    pub(super) message: WireChoiceMessage,
}

/// Assistant message of [`WireChoice`].
#[derive(Deserialize)]
pub(super) struct WireChoiceMessage {
    #[serde(default)]
    pub(super) content: Option<String>,
    #[serde(default)]
    pub(super) tool_calls: Option<Vec<ToolCall>>,
    #[serde(default)]
    pub(super) reasoning_content: Option<String>,
    /// Kimi and Hunyuan send their reasoning text as `reasoning`.
    #[serde(default)]
    pub(super) reasoning: Option<String>,
    #[serde(default)]
    pub(super) reasoning_details: Vec<serde_json::Value>,
    /// Messages `thinking` / `redacted_thinking` blocks (normalized responses only).
    #[serde(default)]
    pub(super) thinking_blocks: Vec<serde_json::Value>,
    /// Responses `reasoning` items (normalized responses only).
    #[serde(default)]
    pub(super) reasoning_items: Vec<serde_json::Value>,
    /// Gemini's reasoning signature (normalized responses only).
    #[serde(default)]
    pub(super) thought_signature: Option<String>,
}

/// Converts SDK messages to the OpenAI wire shape.
///
/// Text becomes a plain message; `ToolUse` becomes an assistant message carrying
/// tool calls; each `ToolResult` entry becomes a `tool`-role message. An image becomes a
/// user message with the image beside its text; only requests to a model that reads images
/// may hold one (see `chat_request`). PDF and image-link inputs are rejected.
pub(super) fn to_wire_messages(messages: &[ChatMessage]) -> Result<Vec<WireMessage>, LLMError> {
    let mut wire = Vec::with_capacity(messages.len());
    for message in messages {
        match &message.message_type {
            MessageType::Text => {
                let role = match &message.role {
                    ChatRole::System => "system",
                    ChatRole::Assistant => "assistant",
                    ChatRole::Tool => "user",
                    ChatRole::User => "user",
                };
                wire.push(WireMessage {
                    role,
                    content: Some(message.content.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                    // An earlier final answer goes back with its own reasoning.
                    reasoning: super::reasoning::WireReasoning::from_kept(
                        (role == "assistant")
                            .then(|| super::reasoning::replay_answer(&message.content))
                            .flatten(),
                    ),
                    image: None,
                });
            }
            MessageType::ToolUse(calls) => wire.push(WireMessage {
                role: "assistant",
                content: if message.content.is_empty() {
                    None
                } else {
                    Some(message.content.clone())
                },
                tool_calls: Some(
                    calls
                        .iter()
                        .cloned()
                        .map(|mut call| {
                            // Rejected calls remain exact in stored history. Only their outgoing
                            // representation changes: replaying invalid JSON poisons the next
                            // request (including Messages' local conversion). Never repair/execute it.
                            if !serde_json::from_str::<serde_json::Value>(&call.function.arguments)
                                .is_ok_and(|args| args.is_object())
                            {
                                call.function.arguments = serde_json::json!({
                                    "_themis_rejected_tool_arguments": call.function.arguments
                                })
                                .to_string();
                            }
                            call
                        })
                        .collect(),
                ),
                tool_call_id: None,
                reasoning: super::reasoning::WireReasoning::from_kept(super::reasoning::replay(
                    calls,
                )),
                image: None,
            }),
            MessageType::ToolResult(results) => {
                for result in results {
                    wire.push(WireMessage {
                        role: "tool",
                        content: Some(result.function.arguments.clone()),
                        tool_calls: None,
                        tool_call_id: Some(result.id.clone()),
                        reasoning: Default::default(),
                        image: None,
                    });
                }
            }
            MessageType::Image((mime, bytes)) => {
                use base64::Engine;
                wire.push(WireMessage {
                    role: "user",
                    content: Some(message.content.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning: Default::default(),
                    image: Some(WireImage {
                        mime: mime.mime_type(),
                        data: base64::engine::general_purpose::STANDARD.encode(bytes),
                    }),
                });
            }
            MessageType::Pdf(_) | MessageType::ImageURL(_) => {
                return Err(LLMError::invalid_request(
                    "CompatibleProvider supports text, image and tool messages only".to_string(),
                ));
            }
        }
    }
    Ok(wire)
}

/// `ChatResponse` implementation returned by [`CompatibleProvider`].
#[derive(Debug)]
pub(super) struct CompatibleChatResponse {
    pub(super) text: Option<String>,
    pub(super) tool_calls: Option<Vec<ToolCall>>,
    pub(super) usage: Option<Usage>,
}

impl ChatResponse for CompatibleChatResponse {
    fn text(&self) -> Option<String> {
        self.text.clone()
    }

    fn tool_calls(&self) -> Option<Vec<ToolCall>> {
        self.tool_calls.clone()
    }

    fn usage(&self) -> Option<Usage> {
        self.usage.clone()
    }
}

impl fmt::Display for CompatibleChatResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(text) = &self.text {
            write!(f, "{text}")?;
        }
        if let Some(calls) = &self.tool_calls {
            for call in calls {
                write!(f, "{call}")?;
            }
        }
        Ok(())
    }
}
