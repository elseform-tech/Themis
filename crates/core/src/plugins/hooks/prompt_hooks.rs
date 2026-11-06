use super::*;

/// The model `prompt` hooks ask, and how many times they still may in this run. A run and its
/// subagents share them.
#[derive(Clone)]
pub struct PromptModel {
    pub llm: Arc<dyn autoagents::llm::LLMProvider>,
    pub calls_left: Arc<std::sync::atomic::AtomicU32>,
    /// `hooks.prompt_hook_calls` is 0: the person turned these off.
    pub off: bool,
}

impl PromptModel {
    #[must_use]
    pub fn new(llm: Arc<dyn autoagents::llm::LLMProvider>, calls: u32) -> Self {
        Self {
            llm,
            calls_left: Arc::new(std::sync::atomic::AtomicU32::new(calls)),
            off: calls == 0,
        }
    }

    pub(super) fn take_call(&self) -> bool {
        use std::sync::atomic::Ordering;
        let mut left = self.calls_left.load(Ordering::SeqCst);
        while left > 0 {
            match self.calls_left.compare_exchange(
                left,
                left - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return true,
                Err(now) => left = now,
            }
        }
        false
    }
}

/// Most of a hook's input a `prompt` hook's request carries, in bytes.
pub(super) const PROMPT_HOOK_INPUT_BYTES: usize = 32 * 1024;
/// Most of a plugin's question kept, in bytes.
pub const PROMPT_HOOK_BYTES: usize = 16 * 1024;

/// What a `prompt` hook's model answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptVerdict {
    pub ok: bool,
    pub reason: Option<String>,
}

/// Reads a `prompt` hook's answer, which is model output and untrusted: one JSON object with `ok`,
/// whole, in a code fence or among other text; or text whose last line is only `approve`, which is
/// how a plugin's question may ask to end. Anything else is `None`, and a guard holds on that.
#[must_use]
pub fn parse_prompt_verdict(reply: &str) -> Option<PromptVerdict> {
    let text = reply.trim();
    let start = text.find('{');
    let end = text.rfind('}');
    let object = match (start, end) {
        (Some(start), Some(end)) if start < end => {
            serde_json::from_str::<Value>(&text[start..=end]).ok()
        }
        _ => None,
    };
    if let Some(ok) = object.as_ref().and_then(|value| value["ok"].as_bool()) {
        let reason = object
            .as_ref()
            .and_then(|value| value["reason"].as_str())
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .map(|reason| capped(reason, BLOCK_REASON_LIMIT).to_owned());
        return Some(PromptVerdict { ok, reason });
    }
    let last = text.lines().rev().find(|line| !line.trim().is_empty())?;
    last.trim()
        .trim_matches(|c: char| c.is_ascii_punctuation())
        .eq_ignore_ascii_case("approve")
        .then_some(PromptVerdict {
            ok: true,
            reason: None,
        })
}
