//! Asking the model for an answer: deadlines, idle limits, and retrying rate limits, cut-offs and transient failures.

use super::*;

pub(super) async fn request_answer(
    llm: &Arc<dyn LLMProvider>,
    messages: &[ChatMessage],
    llm_tools: &[Tool],
    events: &tokio::sync::mpsc::Sender<RunEvent>,
    stopped: &AtomicBool,
) -> anyhow::Result<(Option<String>, Vec<ToolCall>)> {
    request_answer_with_timeout(
        llm,
        messages,
        llm_tools,
        events,
        stopped,
        std::time::Duration::from_secs(300),
    )
    .await
}

/// The output cap after `cut_offs` cut-off replies: doubled each time, up to four times the
/// first cap, but never more than a third of the run's context budget, the ordinary cap's
/// own ceiling. A larger reply does not fit beside a later checkpoint: at 32k memory a
/// 15,109-token write let through at half the budget left the next checkpoint no room.
/// The first attempt keeps `base`.
pub(super) fn retry_output_cap(base: u32, cut_offs: u32) -> u32 {
    let grown = base.saturating_mul(1 << cut_offs.min(2));
    let ceiling = MODEL_REQUEST_BUDGET
        .try_with(|budget| u32::try_from(*budget / 3).unwrap_or(u32::MAX))
        .unwrap_or(u32::MAX);
    grown.min(ceiling.max(base))
}

/// Longest one answer attempt, and all attempts of one answer, may run while the provider
/// keeps sending; idle time is bounded separately (see `unless_idle`).
pub(super) const ATTEMPT_CEILING: std::time::Duration = std::time::Duration::from_secs(3600);

pub(super) const ANSWER_CEILING: std::time::Duration = std::time::Duration::from_secs(3 * 3600);

/// The current answer's stream-progress clock, or a fresh one outside an answer request.
pub(super) fn attempt_progress() -> Arc<std::sync::Mutex<tokio::time::Instant>> {
    crate::providers::STREAM_PROGRESS
        .try_with(Arc::clone)
        .unwrap_or_else(|_| Arc::new(std::sync::Mutex::new(tokio::time::Instant::now())))
}

/// Like `tokio::time::timeout`, but only time without provider data counts: a model that keeps
/// streaming a long reply is not cut off, while a stalled request still times out after `idle`.
/// `ceiling` bounds it absolutely.
pub(super) async fn unless_idle<F: std::future::Future>(
    idle: std::time::Duration,
    ceiling: std::time::Duration,
    progress: Arc<std::sync::Mutex<tokio::time::Instant>>,
    future: F,
) -> Result<F::Output, ()> {
    let started = tokio::time::Instant::now();
    let ceiling = started + ceiling.max(idle);
    let last = || (*progress.lock().unwrap_or_else(|e| e.into_inner())).max(started);
    // On the heap: an answer's state is large, and keeping it in this frame overflows the
    // runtime's worker stacks in debug builds.
    let mut future = Box::pin(future);
    loop {
        let deadline = (last() + idle).min(ceiling);
        tokio::select! {
            output = &mut future => return Ok(output),
            () = tokio::time::sleep_until(deadline) => {
                let now = tokio::time::Instant::now();
                if now >= ceiling || last() + idle <= now {
                    return Err(());
                }
            }
        }
    }
}

pub(super) async fn request_answer_with_timeout(
    llm: &Arc<dyn LLMProvider>,
    messages: &[ChatMessage],
    llm_tools: &[Tool],
    events: &tokio::sync::mpsc::Sender<RunEvent>,
    stopped: &AtomicBool,
    timeout: std::time::Duration,
) -> anyhow::Result<(Option<String>, Vec<ToolCall>)> {
    let progress = Arc::new(std::sync::Mutex::new(tokio::time::Instant::now()));
    until_stopped(
        crate::providers::STREAM_PROGRESS.scope(
            Arc::clone(&progress),
            unless_idle(
                timeout.saturating_mul(3),
                ANSWER_CEILING,
                Arc::clone(&progress),
                request_answer_inner(llm, messages, llm_tools, events, stopped, timeout),
            ),
        ),
        stopped,
        events,
    )
    .await?
    .map_err(|_| anyhow!("LLM answer deadline exceeded"))?
}

/// Answer retries after HTTP 429: waits of about 10, 20, 40, 80 and 160 seconds (310 s in all).
pub(super) const RATE_LIMIT_RETRIES: u32 = 5;

pub(super) const RATE_LIMIT_BACKOFF_SECONDS: u64 = 10;

/// Answer retries after a cut-off (truncated) provider response.
pub(super) const TRUNCATED_RETRIES: u32 = 4;

pub(super) fn answer_retry_limit(rate_limited: bool, truncated: bool) -> u32 {
    if rate_limited {
        RATE_LIMIT_RETRIES
    } else if truncated {
        TRUNCATED_RETRIES
    } else {
        2
    }
}

// Retry-After is a floor, never shortened by jitter. Separate randomized states
// keep simultaneous clients from retrying in lockstep; no provider data is hashed.
pub(super) fn request_retry_delay(
    error: &autoagents::llm::error::LLMError,
    retry: u32,
) -> std::time::Duration {
    request_retry_delay_with_rate_limit_floor(error, retry, 1)
}

pub(super) fn request_retry_delay_with_rate_limit_floor(
    error: &autoagents::llm::error::LLMError,
    retry: u32,
    rate_limit_seconds: u64,
) -> std::time::Duration {
    use autoagents::llm::error::LLMError;
    use std::time::Duration;
    let seconds = if error.http_status_code() == Some(429) {
        rate_limit_seconds
    } else {
        1
    };
    let base = Duration::from_secs(seconds.saturating_mul(1_u64 << retry.min(6)));
    let hint = match error {
        LLMError::RateLimitError { retry_after, .. }
        | LLMError::HttpStatusError { retry_after, .. } => retry_after.unwrap_or_default(),
        _ => Duration::ZERO,
    };
    let jitter = std::collections::hash_map::RandomState::new().hash_one(retry)
        % (base.as_millis() as u64 / 2 + 1);
    base.max(hint).saturating_add(Duration::from_millis(jitter))
}

pub(super) async fn request_answer_inner(
    llm: &Arc<dyn LLMProvider>,
    messages: &[ChatMessage],
    llm_tools: &[Tool],
    events: &tokio::sync::mpsc::Sender<RunEvent>,
    stopped: &AtomicBool,
    attempt_timeout: std::time::Duration,
) -> anyhow::Result<(Option<String>, Vec<ToolCall>)> {
    const EMPTY_ANSWER: &str = "Provider returned no public answer or tool calls";
    let mut retries = 0;
    // Each cut-off doubles the output cap for the next attempt (up to four times it, and the
    // model's own limit): a thinking model's reasoning counts toward the cap, so retrying a
    // long reply at the same cap is cut off again.
    let base_limit = crate::providers::OUTPUT_LIMIT.try_with(|limit| *limit).ok();
    let mut cut_offs = 0u32;
    // A silent cut-off (nothing shown, so likely one huge tool call such as a whole file in
    // one write) gets one note to split the work once a larger cap cannot help: when the cap
    // is already at its ceiling, or from the second cut-off.
    let mut split_noted = false;
    // After an empty reply the retry says so, instead of resending the same request.
    let mut request = std::borrow::Cow::Borrowed(messages);
    // Text already shown from replies that were cut off; the answer continues after it.
    let mut shown = String::new();
    let mut repeat = SENT_BACK_REPLY
        .try_with(|reply| reply.as_deref().map(turn_end::RepeatFilter::new))
        .ok()
        .flatten();
    loop {
        if let Some(filter) = &mut repeat {
            filter.restart();
        }
        let mut published = false;
        let mut attempt_text = String::new();
        let mut written_calls = turn_end::WrittenCallFilter::default();
        let messages: &[ChatMessage] = &request;
        let attempt = crate::context_metrics::with_attempt(retries as usize, async {
            // Bound handshake, complete stream and non-streaming fallback together.
            // The published flag survives cancellation so partial text cannot be replayed.
            unless_idle(
                attempt_timeout,
                ATTEMPT_CEILING,
                attempt_progress(),
                async {
                    match llm
                        .chat_stream_with_tools(messages, Some(llm_tools), None)
                        .await
                    {
                        Ok(mut stream) => {
                            let mut text = String::new();
                            let mut calls = Vec::new();
                            while let Some(chunk) = stream.next().await {
                                check_stopped(stopped, events).await?;
                                match chunk? {
                                    StreamChunk::Text(delta) => {
                                        attempt_text.push_str(&delta);
                                        text.push_str(&delta);
                                        let delta = written_calls.push(&delta);
                                        if delta.is_empty() {
                                            continue;
                                        }
                                        let visible = match &mut repeat {
                                            Some(filter) => filter.push(&delta),
                                            None => Some(delta),
                                        };
                                        if let Some(visible) = visible {
                                            published |= !visible.is_empty();
                                            events
                                                .send(RunEvent::AssistantText(visible))
                                                .await
                                                .ok();
                                        }
                                    }
                                    StreamChunk::ToolUseComplete { tool_call, .. } => {
                                        calls.push(tool_call)
                                    }
                                    _ => {} // Reasoning content is private; only public narration reaches the UI.
                                }
                            }
                            // What was held back in case it was a call wasn't one.
                            let rest = written_calls.finish();
                            if !rest.is_empty() {
                                let visible = match &mut repeat {
                                    Some(filter) => filter.push(&rest),
                                    None => Some(rest),
                                };
                                if let Some(visible) = visible {
                                    published |= !visible.is_empty();
                                    events.send(RunEvent::AssistantText(visible)).await.ok();
                                }
                            }
                            if let Some(held) = repeat.as_mut().and_then(|f| f.finish()) {
                                published = true;
                                events.send(RunEvent::AssistantText(held)).await.ok();
                            }
                            Ok(((!text.is_empty()).then_some(text), calls))
                        }
                        Err(autoagents::llm::error::LLMError::Generic(message))
                            if message
                                == "Streaming with tools not supported for this provider" =>
                        {
                            let response =
                                llm.chat_with_tools(messages, Some(llm_tools), None).await?;
                            let text = response.text().filter(|text| !text.is_empty());
                            if let Some(text) = text.clone() {
                                attempt_text.push_str(&text);
                                let text = turn_end::before_written_call(&text);
                                let visible = match &mut repeat {
                                    Some(filter) => {
                                        let mut visible = filter.push(&text).unwrap_or_default();
                                        visible.extend(filter.finish());
                                        visible
                                    }
                                    None => text,
                                };
                                if !visible.is_empty() {
                                    published = true;
                                    events.send(RunEvent::AssistantText(visible)).await.ok();
                                }
                            }
                            Ok((text, response.tool_calls().unwrap_or_default()))
                        }
                        Err(error) => Err(error.into()),
                    }
                },
            )
            .await
            .map_err(|_| {
                autoagents::llm::error::LLMError::HttpError(
                    "request timed out: ordinary attempt deadline exceeded".into(),
                )
            })?
        });
        let result: anyhow::Result<(Option<String>, Vec<ToolCall>)> = match base_limit {
            Some(base) => {
                crate::providers::OUTPUT_LIMIT
                    .scope(retry_output_cap(base, cut_offs), attempt)
                    .await
            }
            None => attempt.await,
        }
        .and_then(|(text, calls)| {
            if calls.is_empty() && text.as_deref().is_none_or(|text| text.trim().is_empty()) {
                Err(autoagents::llm::error::LLMError::Generic(EMPTY_ANSWER.into()).into())
            } else {
                Ok((text, calls))
            }
        });
        let Some(error) = result
            .as_ref()
            .err()
            .and_then(|error| error.downcast_ref::<autoagents::llm::error::LLMError>())
        else {
            return result.map(|(text, calls)| (with_shown(&shown, text), calls));
        };
        let empty_output = matches!(error, autoagents::llm::error::LLMError::Generic(message) if message == EMPTY_ANSWER);
        let truncated = matches!(error, autoagents::llm::error::LLMError::Generic(message) if message == "Provider response was truncated");
        // A stream that dropped or failed part-way is as temporary as a transport error.
        let stream_dropped = matches!(error, autoagents::llm::error::LLMError::Generic(message)
            if message == "Provider stream ended before completion"
                || message == "Provider response failed or was incomplete"
                || message.starts_with("Provider stream error"));
        // A rate limit or a cut-off response usually clears with time: losing a long run to
        // it costs far more than waiting, so those get more and longer retries (still inside
        // the overall answer deadline).
        let rate_limited = error.http_status_code() == Some(429);
        // A used-up quota does not clear in minutes: waiting out the retries only delays
        // the failure (a goal then pauses and retries on its own schedule).
        let quota_used_up = rate_limited
            && crate::goal::classify_failure(&error.to_string()) == crate::goal::Failure::Quota;
        // Shown text cannot be taken back, so a reply cut off after it continues from it;
        // any other failure after it ends the answer.
        if quota_used_up
            || retries >= answer_retry_limit(rate_limited, truncated)
            || (published && !truncated)
            || (!transient(error) && !empty_output && !truncated && !stream_dropped)
        {
            return result;
        }
        if truncated {
            cut_offs += 1;
        }
        if published {
            let mut retry = request.into_owned();
            retry.push(ChatMessage {
                role: ChatRole::Assistant,
                message_type: MessageType::Text,
                content: attempt_text.clone(),
            });
            retry.push(ChatMessage {
                role: ChatRole::User,
                message_type: MessageType::Text,
                content: CUT_OFF_NOTE.to_owned(),
            });
            request = std::borrow::Cow::Owned(retry);
            shown = with_shown(&shown, Some(attempt_text)).unwrap_or_default();
            events
                .send(RunEvent::AssistantText("\n\n".into()))
                .await
                .ok();
        }
        let cap_cannot_grow = base_limit.is_some_and(|base| {
            retry_output_cap(base, cut_offs) <= retry_output_cap(base, cut_offs.saturating_sub(1))
        });
        if truncated && !published && !split_noted && (cut_offs >= 2 || cap_cannot_grow) {
            split_noted = true;
            let mut retry = request.into_owned();
            retry.push(ChatMessage {
                role: ChatRole::User,
                message_type: MessageType::Text,
                content: SILENT_CUT_OFF_NOTE.to_owned(),
            });
            request = std::borrow::Cow::Owned(retry);
        }
        if empty_output {
            let mut retry = request.into_owned();
            retry.push(ChatMessage {
                role: ChatRole::User,
                message_type: MessageType::Text,
                content: EMPTY_ANSWER_NOTE.to_owned(),
            });
            request = std::borrow::Cow::Owned(retry);
        }
        // Calls from an interrupted response have not been executed; completed tools are already in messages.
        let delay = if rate_limited {
            request_retry_delay_with_rate_limit_floor(error, retries, RATE_LIMIT_BACKOFF_SECONDS)
        } else {
            request_retry_delay(error, retries)
        };
        retries += 1;
        crate::diagnostics::emit(
            "answer",
            "request_retry",
            "warn",
            serde_json::json!({
                "retry": retries, "status": error.http_status_code(), "backoff_ms": delay.as_millis(),
                "reason": if empty_output { "empty_output" } else if truncated { "output_truncated" } else if rate_limited { "rate_limited" } else if stream_dropped { "stream_dropped" } else { "transient" },
            }),
        );
        eprintln!(
            "answer retry={retries} status={:?} backoff_ms={}",
            error.http_status_code(),
            delay.as_millis()
        );
        tokio::time::sleep(delay).await;
    }
}
