//! Per-run correction of the byte-based token estimate using provider-reported
//! input usage, a cached message-size estimator, and context-overflow error
//! classification. Holds numbers only; no prompt, tool payload or provider body
//! is retained.
use autoagents::llm::chat::{ChatMessage, ChatRole, MessageType};
use autoagents::llm::error::LLMError;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

pub const MIN_RATIO: f64 = 0.75;
pub const MAX_RATIO: f64 = 4.0;
const RAW_BUDGET_FLOOR: usize = 256;
const SMOOTHING: f64 = 0.5;
const TIGHTEN_FACTOR: f64 = 1.25;
const TIGHTEN_MARGIN: f64 = 1.05;

/// Correction ratio = provider-reported input tokens / raw estimated input
/// tokens (messages plus tool definitions). Without an observation the ratio is
/// absent and every estimate is unchanged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenCalibration {
    ratio: Option<f64>,
    /// The next observation replaces the ratio instead of being smoothed in
    /// (initially, and after an overflow tightened the ratio).
    replace_next: bool,
}

impl Default for TokenCalibration {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenCalibration {
    pub fn new() -> Self {
        Self {
            ratio: None,
            replace_next: true,
        }
    }

    pub fn ratio(&self) -> Option<f64> {
        self.ratio
    }

    /// Multiplier applied to estimates (1.0 before any observation).
    pub fn factor(&self) -> f64 {
        self.ratio.unwrap_or(1.0)
    }

    /// Records one request. Missing or zero usage leaves the ratio unchanged.
    /// Returns the unsmoothed, clamped ratio of this request.
    pub fn observe(&mut self, reported_input: u64, raw_estimate: usize) -> Option<f64> {
        if reported_input == 0 || raw_estimate == 0 {
            return None;
        }
        let observed = (reported_input as f64 / raw_estimate as f64).clamp(MIN_RATIO, MAX_RATIO);
        let next = match self.ratio {
            Some(previous) if !self.replace_next => {
                previous * (1.0 - SMOOTHING) + observed * SMOOTHING
            }
            _ => observed,
        };
        self.ratio = Some(next.clamp(MIN_RATIO, MAX_RATIO));
        self.replace_next = false;
        Some(observed)
    }

    /// Calibrated message tokens. The ratio was measured over messages plus
    /// tool definitions (`schema`), so scale the sum and subtract `schema`.
    pub fn messages(&self, raw_messages: usize, schema: usize) -> usize {
        match self.ratio {
            None => raw_messages,
            Some(ratio) => (((raw_messages.saturating_add(schema)) as f64 * ratio).round()
                as usize)
                .saturating_sub(schema),
        }
    }

    /// The raw-estimate budget equivalent to a budget in calibrated tokens.
    pub fn raw_budget(&self, calibrated_budget: usize, schema: usize) -> usize {
        match self.ratio {
            None => calibrated_budget,
            Some(ratio) => {
                let raw = (((calibrated_budget.saturating_add(schema)) as f64 / ratio).floor()
                    as usize)
                    .saturating_sub(schema);
                // Never collapse to zero: keep a small usable window.
                raw.max(
                    RAW_BUDGET_FLOOR
                        .min(calibrated_budget)
                        .max(calibrated_budget / 8),
                )
            }
        }
    }

    /// A context overflow proved the request was larger than estimated: raise
    /// the ratio so the request counts as over `threshold` (calibrated tokens),
    /// which makes the next budget check compact it.
    pub fn tighten(&mut self, raw_messages: usize, schema: usize, threshold: usize) {
        let needed = (threshold.saturating_add(schema)) as f64 * TIGHTEN_MARGIN
            / raw_messages.saturating_add(schema).max(1) as f64;
        let raised = (self.factor() * TIGHTEN_FACTOR).max(needed);
        self.ratio = Some(raised.clamp(MIN_RATIO, MAX_RATIO));
        self.replace_next = true;
    }
}

fn role_tag(role: &ChatRole) -> u8 {
    match role {
        ChatRole::System => 0,
        ChatRole::User => 1,
        ChatRole::Assistant => 2,
        ChatRole::Tool => 3,
    }
}

/// Content fingerprint used only as a cache key; `None` for media payloads,
/// which are measured directly.
fn fingerprint(message: &ChatMessage) -> Option<u64> {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    role_tag(&message.role).hash(&mut hash);
    message.content.hash(&mut hash);
    match &message.message_type {
        MessageType::Text => 0u8.hash(&mut hash),
        MessageType::ImageURL(url) => {
            1u8.hash(&mut hash);
            url.hash(&mut hash);
        }
        MessageType::ToolUse(calls) | MessageType::ToolResult(calls) => {
            let kind = matches!(message.message_type, MessageType::ToolUse(_)) as u8 + 2;
            kind.hash(&mut hash);
            calls.len().hash(&mut hash);
            for call in calls {
                call.id.hash(&mut hash);
                call.call_type.hash(&mut hash);
                call.function.name.hash(&mut hash);
                call.function.arguments.hash(&mut hash);
            }
        }
        MessageType::Image(_) | MessageType::Pdf(_) => return None,
    }
    Some(hash.finish())
}

#[derive(Default)]
struct CountingWriter(usize);

impl std::io::Write for CountingWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0 += buffer.len();
        Ok(buffer.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn serialized_len(message: &ChatMessage) -> usize {
    let mut counter = CountingWriter::default();
    match serde_json::to_writer(&mut counter, message) {
        Ok(()) => counter.0,
        Err(_) => 0,
    }
}

const CACHE_LIMIT: usize = 8192;
thread_local! { static SIZE_CACHE: RefCell<HashMap<u64, usize>> = RefCell::new(HashMap::new()); }

/// Raw estimate of one message: serialized bytes / 4 plus 16 per message.
/// Unchanged messages are measured once; the cache keys on content, so
/// edited messages always re-measure.
pub fn message_tokens(message: &ChatMessage) -> usize {
    let Some(key) = fingerprint(message) else {
        return serialized_len(message).div_ceil(4) + 16;
    };
    SIZE_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(size) = cache.get(&key) {
            return *size;
        }
        let size = serialized_len(message).div_ceil(4) + 16;
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key, size);
        size
    })
}

const OVERFLOW_MARKERS: &[&str] = &[
    "context_length_exceeded",
    "context length",
    "context window",
    "maximum context",
    "prompt is too long",
    "input is too long",
    "too many tokens",
    "reduce the length of the messages",
    "request_too_large",
    "exceeds the maximum allowed number of bytes",
    "exceeds the maximum number of tokens",
    "input token count",
];
const OVERFLOW_STATUSES: &[u16] = &[400, 413, 422];
const INSPECTED_BYTES: usize = 16 * 1024;

/// Conservative context-length-exceeded test: only client-error statuses that
/// providers use for oversized input, and only well-known message fragments or
/// the OpenAI-compatible error code. Inspects a bounded prefix.
pub fn is_context_overflow(status: Option<u16>, body: &str) -> bool {
    if !status.is_some_and(|status| OVERFLOW_STATUSES.contains(&status)) {
        return false;
    }
    let mut end = body.len().min(INSPECTED_BYTES);
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    let body = body[..end].to_lowercase();
    OVERFLOW_MARKERS.iter().any(|marker| body.contains(marker))
}

/// Classifies a provider error as a context overflow.
pub fn error_is_context_overflow(error: &LLMError) -> bool {
    match error {
        LLMError::HttpStatusError {
            status_code,
            message,
            response_body,
            provider_code,
            ..
        } => {
            is_context_overflow(Some(*status_code), response_body)
                || is_context_overflow(Some(*status_code), message)
                || provider_code
                    .as_deref()
                    .is_some_and(|code| is_context_overflow(Some(*status_code), code))
        }
        LLMError::InvalidRequest {
            message,
            status_code,
            response_body,
        } => {
            is_context_overflow(*status_code, message)
                || is_context_overflow(*status_code, response_body.as_deref().unwrap_or_default())
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn no_observation_leaves_estimates_unchanged() {
        let calibration = TokenCalibration::new();
        assert_eq!(calibration.messages(1234, 99), 1234);
        assert_eq!(calibration.raw_budget(777, 99), 777);
    }

    #[test]
    fn first_observation_sets_ratio_and_later_ones_are_smoothed() {
        let mut calibration = TokenCalibration::new();
        calibration.observe(2000, 1000);
        assert_eq!(calibration.ratio(), Some(2.0));
        calibration.observe(1000, 1000);
        assert_eq!(calibration.ratio(), Some(1.5));
        assert_eq!(calibration.messages(900, 100), 1400);
        assert_eq!(calibration.raw_budget(1400, 100), 900);
    }

    #[test]
    fn ratio_is_clamped_and_missing_usage_is_ignored() {
        let mut calibration = TokenCalibration::new();
        assert_eq!(calibration.observe(0, 1000), None);
        assert_eq!(calibration.observe(5, 0), None);
        assert_eq!(calibration.ratio(), None);
        calibration.observe(1_000_000, 10);
        assert_eq!(calibration.ratio(), Some(MAX_RATIO));
        calibration.tighten(10, 0, 1_000_000);
        assert_eq!(calibration.ratio(), Some(MAX_RATIO));
        let mut low = TokenCalibration::new();
        low.observe(1, 1000);
        assert_eq!(low.ratio(), Some(MIN_RATIO));
        assert_eq!(MIN_RATIO, 0.75);
    }

    #[test]
    fn raw_budget_keeps_a_floor_when_schema_dominates() {
        let mut calibration = TokenCalibration::new();
        calibration.observe(1_000_000, 10);
        // (1000 + 5000) / 4 - 5000 saturates to zero without a floor.
        assert_eq!(calibration.raw_budget(1000, 5000), 256);
        assert_eq!(calibration.raw_budget(100, 5000), 100);
        assert_eq!(calibration.raw_budget(16_000, 20_000), 2000);
    }

    #[test]
    fn tighten_makes_the_failed_request_exceed_the_threshold_then_resets_on_observation() {
        let mut calibration = TokenCalibration::new();
        calibration.tighten(1000, 200, 1000);
        assert!(calibration.messages(1000, 200) > 1000);
        calibration.observe(1500, 1200);
        assert_eq!(calibration.ratio(), Some(1.25));
    }

    #[test]
    fn cached_estimate_matches_the_serialized_definition() {
        use autoagents::llm::chat::MessageType;
        use autoagents::llm::{FunctionCall, ToolCall};
        let call = ToolCall {
            id: "c".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: "{\"path\":\"a\"}".into(),
            },
        };
        let messages = [
            ChatMessage {
                role: ChatRole::User,
                message_type: MessageType::Text,
                content: "héllo \"quoted\" \n".into(),
            },
            ChatMessage {
                role: ChatRole::Assistant,
                message_type: MessageType::ToolUse(vec![call.clone()]),
                content: String::new(),
            },
            ChatMessage {
                role: ChatRole::Tool,
                message_type: MessageType::ToolResult(vec![call]),
                content: String::new(),
            },
        ];
        for _ in 0..2 {
            for message in &messages {
                let expected = serde_json::to_string(message).unwrap().len().div_ceil(4) + 16;
                assert_eq!(message_tokens(message), expected);
            }
        }
    }

    #[test]
    fn classifier_matches_openai_and_common_messages_only_on_client_errors() {
        let openai = r#"{"error":{"code":"context_length_exceeded","message":"This model's maximum context length is 8192 tokens."}}"#;
        assert!(is_context_overflow(Some(400), openai));
        assert!(is_context_overflow(
            Some(413),
            "Prompt is too long: 250000 tokens"
        ));
        assert!(!is_context_overflow(Some(500), openai));
        assert!(!is_context_overflow(None, openai));
        assert!(!is_context_overflow(
            Some(400),
            r#"{"error":"invalid tool schema"}"#
        ));
        assert!(!is_context_overflow(
            Some(429),
            "too many tokens per minute"
        ));
    }

    proptest! {
        #[test]
        fn classifier_never_panics_and_ignores_non_overflow_statuses(
            status in proptest::option::of(any::<u16>()),
            body in ".{0,400}",
        ) {
            let result = is_context_overflow(status, &body);
            if !status.is_some_and(|status| OVERFLOW_STATUSES.contains(&status)) {
                prop_assert!(!result);
            }
        }

        #[test]
        fn json_like_bodies_without_prose_are_not_overflows(body in r#"[0-9{}\[\]",:_ ]{0,300}"#) {
            prop_assert!(!is_context_overflow(Some(400), &body));
        }

        #[test]
        fn embedded_marker_is_detected_wherever_it_appears(
            prefix in ".{0,200}",
            suffix in ".{0,200}",
            index in 0..OVERFLOW_MARKERS.len(),
        ) {
            let body = format!("{prefix}{}{suffix}", OVERFLOW_MARKERS[index].to_uppercase());
            // Only a marker inside the inspected prefix is guaranteed visible.
            prop_assume!(prefix.len() + OVERFLOW_MARKERS[index].len() <= INSPECTED_BYTES);
            prop_assert!(is_context_overflow(Some(400), &body));
        }

        #[test]
        fn ratio_stays_bounded_and_estimates_never_panic(
            observations in proptest::collection::vec((any::<u64>(), 0usize..10_000_000), 0..20),
            raw in 0usize..10_000_000,
            schema in 0usize..100_000,
        ) {
            let mut calibration = TokenCalibration::new();
            for (reported, estimated) in observations {
                calibration.observe(reported, estimated);
                if let Some(ratio) = calibration.ratio() {
                    prop_assert!((MIN_RATIO..=MAX_RATIO).contains(&ratio));
                }
            }
            let _ = calibration.messages(raw, schema);
            let _ = calibration.raw_budget(raw, schema);
        }
    }
}
