//! Content-free request accounting. Provider subcounts never inflate token totals.
use crate::configuration::ContextMetricsLevel;
use autoagents::llm::chat::{ChatMessage, ChatRole, MessageType, Tool};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

tokio::task_local! { static ACTIVE: Arc<Mutex<Metrics>>; }
tokio::task_local! { static LEVEL: ContextMetricsLevel; }
tokio::task_local! { static STAGE: (String, usize); }
tokio::task_local! { static ATTEMPT: usize; }
tokio::task_local! { static USAGE_SINK: Arc<std::sync::atomic::AtomicU64>; }
tokio::task_local! { static CONFIG_FINGERPRINT: String; }

#[derive(Default)]
struct Metrics {
    level: ContextMetricsLevel,
    requests: Vec<Value>,
    total_requests: usize,
    input: u64,
    output: u64,
    known_input: usize,
    known_output: usize,
    failed: usize,
    run_metadata: Value,
    activities: std::collections::BTreeMap<String, u64>,
    calibration: Vec<Value>,
}

pub async fn with_level<F: std::future::Future>(
    level: ContextMetricsLevel,
    future: F,
) -> F::Output {
    LEVEL.scope(level, future).await
}
pub async fn with_run_policy<F: std::future::Future>(
    config: &crate::configuration::RuntimeConfig,
    future: F,
) -> F::Output {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(config)
        .unwrap_or_default()
        .hash(&mut hash);
    CONFIG_FINGERPRINT
        .scope(
            format!("{:016x}", hash.finish()),
            with_level(config.logging.context_metrics, future),
        )
        .await
}
pub fn config_fingerprint() -> Option<String> {
    CONFIG_FINGERPRINT.try_with(Clone::clone).ok()
}
pub async fn with_stage<F: std::future::Future>(job: &str, stage: usize, future: F) -> F::Output {
    STAGE.scope((job.to_owned(), stage), future).await
}
pub async fn with_attempt<F: std::future::Future>(attempt: usize, future: F) -> F::Output {
    ATTEMPT.scope(attempt, future).await
}

pub async fn collect<F: std::future::Future>(future: F) -> F::Output {
    if ACTIVE.try_with(|_| ()).is_ok() {
        return future.await;
    }
    let level = LEVEL.try_with(|v| *v).unwrap_or_default();
    ACTIVE
        .scope(
            Arc::new(Mutex::new(Metrics {
                level,
                ..Default::default()
            })),
            future,
        )
        .await
}

/// Receives the latest provider-reported input tokens of requests made inside
/// `future`, independent of the metrics level.
pub async fn with_usage_sink<F: std::future::Future>(
    sink: Arc<std::sync::atomic::AtomicU64>,
    future: F,
) -> F::Output {
    USAGE_SINK.scope(sink, future).await
}

/// Keeps one content-free estimate-vs-report sample per ordinary request.
pub fn calibration_sample(sample: Value) {
    let _ = ACTIVE.try_with(|metrics| {
        let mut metrics = metrics.lock().unwrap_or_else(|e| e.into_inner());
        if metrics.calibration.len() == 32 {
            metrics.calibration.remove(0);
        }
        metrics.calibration.push(sample);
    });
}

pub fn set_run_metadata(value: Value) {
    let _ = ACTIVE.try_with(|metrics| {
        metrics
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .run_metadata = value
    });
}
pub fn activity(name: &str) {
    let _ = ACTIVE.try_with(|metrics| {
        *metrics
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .activities
            .entry(name.to_owned())
            .or_default() += 1
    });
}

pub fn snapshot() -> Option<Value> {
    ACTIVE.try_with(|metrics| {
        let metrics = metrics.lock().unwrap_or_else(|e|e.into_inner());
        if metrics.level == ContextMetricsLevel::Off { return None; }
        Some(json!({"pipeline_version":"adaptive-context-v1","measurement":"byte_estimate_and_provider_report",
            "run":metrics.run_metadata,"activities":metrics.activities,"token_calibration":metrics.calibration,"total_requests":metrics.total_requests,"failed_or_cancelled_requests":metrics.failed,
            "provider_input_tokens":if metrics.known_input>0 {Some(metrics.input)} else {None},
            "provider_output_tokens":if metrics.known_output>0 {Some(metrics.output)} else {None},
            "requests_with_input_usage":metrics.known_input,"requests_with_output_usage":metrics.known_output,
            "requests_missing_input_usage":metrics.total_requests-metrics.known_input,
            "requests_missing_output_usage":metrics.total_requests-metrics.known_output,
            "recent_requests":if metrics.level==ContextMetricsLevel::Detailed {metrics.requests.clone()} else {metrics.requests.last().cloned().into_iter().collect()},"recent_request_limit":if metrics.level==ContextMetricsLevel::Detailed {32} else {1},
            "subcounts_included_in_parent_totals":true}))
    }).ok().flatten()
}

#[derive(Default, serde::Serialize)]
struct Bucket {
    bytes: usize,
    message_count: usize,
    estimated_tokens: usize,
}

pub struct RequestTrace {
    collector: Option<Arc<Mutex<Metrics>>>,
    record: Value,
    started: std::time::Instant,
}

impl RequestTrace {
    pub fn begin(
        model: &str,
        messages: &[ChatMessage],
        tools: Option<&[Tool]>,
        summary: bool,
    ) -> Self {
        let collector = ACTIVE.try_with(Arc::clone).ok().filter(|c| {
            c.lock().unwrap_or_else(|e| e.into_inner()).level != ContextMetricsLevel::Off
        });
        let started = std::time::Instant::now();
        if collector.is_none() {
            return Self {
                collector,
                record: Value::Null,
                started,
            };
        }
        let detailed = collector.as_ref().is_some_and(|c| {
            c.lock().unwrap_or_else(|e| e.into_inner()).level == ContextMetricsLevel::Detailed
        });
        let mut roles = std::collections::BTreeMap::<&str, Bucket>::new();
        let mut purposes = std::collections::BTreeMap::<&str, Bucket>::new();
        let mut bytes = 0;
        for (index, message) in messages.iter().enumerate() {
            let size = serde_json::to_vec(message).map_or(0, |b| b.len());
            bytes += size;
            let role = match message.role {
                ChatRole::System => "system",
                ChatRole::User => "user",
                ChatRole::Assistant => "assistant",
                ChatRole::Tool => "tool",
            };
            let role_bucket = roles.entry(role).or_default();
            role_bucket.bytes += size;
            role_bucket.message_count += 1;
            role_bucket.estimated_tokens += size.div_ceil(4) + 16;
            let purpose = if summary {
                "summary_input"
            } else if message.role == ChatRole::System {
                "instructions"
            } else if message.content.starts_with("Earlier context checkpoint") {
                "checkpoint"
            } else if message.content.starts_with("Document semantic capsule") {
                "document_capsule"
            } else if message.content.starts_with("Attached ") {
                "attachment"
            } else if message.content.starts_with("Original exchange anchors") {
                "historical_recall"
            } else if matches!(message.message_type, MessageType::ToolUse(_)) {
                "tool_invocation"
            } else if matches!(message.message_type, MessageType::ToolResult(_)) {
                "tool_observation"
            } else if index + 1 == messages.len() && message.role == ChatRole::User {
                "current_request"
            } else {
                "conversation_history"
            };
            let purpose_bucket = purposes.entry(purpose).or_default();
            purpose_bucket.bytes += size;
            purpose_bucket.message_count += 1;
            purpose_bucket.estimated_tokens += size.div_ceil(4) + 16;
        }
        let schema = tools
            .and_then(|t| serde_json::to_vec(t).ok())
            .map_or(0, |b| b.len());
        let ordinal = collector.as_ref().map(|c| {
            let mut c = c.lock().unwrap_or_else(|e| e.into_inner());
            c.total_requests += 1;
            c.total_requests
        });
        let run = collector.as_ref().map(|c| {
            c.lock()
                .unwrap_or_else(|e| e.into_inner())
                .run_metadata
                .clone()
        });
        let stage = STAGE.try_with(Clone::clone).ok();
        let summary_kind = summary.then(|| {
            if messages
                .first()
                .is_some_and(|m| m.content.contains("You summarize an attached document"))
            {
                "document_capsule"
            } else {
                "context_checkpoint"
            }
        });
        let record = json!({"job_id":stage.as_ref().map(|s|&s.0),"stage":stage.as_ref().map(|s|s.1),"attempt":ATTEMPT.try_with(|a|*a).unwrap_or(0),"run":run,"request_id":ordinal,"request_ordinal":ordinal,"model":model,"purpose":if summary {"summarization"} else {"ordinary"},"summary_kind":summary_kind,
            "message_count":messages.len(),"message_bytes":bytes,"schema_bytes":schema,
            "estimated_input_tokens":(bytes+schema).div_ceil(4)+messages.len()*16,
            "estimate_source":"serialized_bytes_div4_plus_message_overhead","status":"failed_or_cancelled",
            "role_distribution":if detailed {Some(roles)} else {None},"purpose_distribution":if detailed {Some(purposes)} else {None},
            "unattributed_bytes":0,"accounting_duration_us":started.elapsed().as_micros() as u64,"distribution_views_are_alternatives":true,
            "input_tokens":null,"output_tokens":null,"cached_input_tokens":null,"reasoning_output_tokens":null});
        Self {
            collector,
            record,
            started,
        }
    }
    pub fn output_limit(&mut self, limit: Option<u32>) {
        if self.collector.is_some() {
            self.record["output_limit_tokens"] = limit.map(Value::from).unwrap_or(Value::Null);
        }
    }
    pub fn usage(&mut self, usage: &Value) {
        for (target, paths) in [
            ("input_tokens", vec!["/prompt_tokens", "/input_tokens"]),
            (
                "output_tokens",
                vec!["/completion_tokens", "/output_tokens"],
            ),
            (
                "cached_input_tokens",
                vec![
                    "/prompt_tokens_details/cached_tokens",
                    "/input_tokens_details/cached_tokens",
                    "/cache_read_input_tokens",
                ],
            ),
            (
                "reasoning_output_tokens",
                vec![
                    "/completion_tokens_details/reasoning_tokens",
                    "/output_tokens_details/reasoning_tokens",
                ],
            ),
        ] {
            if let Some(value) = paths
                .iter()
                .find_map(|path| usage.pointer(path).and_then(Value::as_u64))
            {
                self.record[target] = value.into();
            }
        }
        // Anthropic reports uncached input separately from cache reads/creation.
        // OpenAI cache-detail fields are already included in input_tokens.
        if usage.get("cache_read_input_tokens").is_some()
            || usage.get("cache_creation_input_tokens").is_some()
        {
            if let Some(base) = usage["input_tokens"].as_u64() {
                let cached = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
                let created = usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                self.record["input_tokens"] = (base + cached + created).into();
                self.record["uncached_input_tokens"] = base.into();
                self.record["cache_creation_input_tokens"] = created.into();
                self.record["input_usage_normalization"] =
                    "anthropic_uncached_plus_cache_read_and_creation".into();
            }
        }
        if let Some(input) = self.record["input_tokens"].as_u64() {
            let _ =
                USAGE_SINK.try_with(|sink| sink.store(input, std::sync::atomic::Ordering::SeqCst));
        }
    }
    pub fn complete(&mut self) {
        self.record["status"] = "completed".into();
    }
}

impl Drop for RequestTrace {
    fn drop(&mut self) {
        let Some(collector) = &self.collector else {
            return;
        };
        self.record["duration_ms"] = (self.started.elapsed().as_millis() as u64).into();
        if let (Some(estimated), Some(actual)) = (
            self.record["estimated_input_tokens"].as_i64(),
            self.record["input_tokens"].as_i64(),
        ) {
            self.record["estimate_error_tokens"] = (estimated - actual).into();
        }
        crate::diagnostics::emit("context", "request_usage", "info", self.record.clone());
        let mut metrics = collector.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = self.record["input_tokens"].as_u64() {
            metrics.input += n;
            metrics.known_input += 1;
        }
        if let Some(n) = self.record["output_tokens"].as_u64() {
            metrics.output += n;
            metrics.known_output += 1;
        }
        if self.record["status"] != "completed" {
            metrics.failed += 1;
        }
        if metrics.requests.len() == 32 {
            metrics.requests.remove(0);
        }
        metrics.requests.push(self.record.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn anthropic_cache_components_are_included_once() {
        collect(async {
            let mut request = RequestTrace::begin("anthropic-model", &[], None, false);
            request.usage(&json!({"input_tokens":10,"cache_read_input_tokens":80,"cache_creation_input_tokens":20,"output_tokens":5}));
            request.usage(&json!({"output_tokens":7}));
            request.complete();
            drop(request);
            assert_eq!(snapshot().unwrap()["provider_input_tokens"],110);
            assert_eq!(snapshot().unwrap()["provider_output_tokens"],7);
        }).await;
    }

    #[tokio::test]
    async fn totals_keep_missing_usage_unknown_and_do_not_add_cache_or_reasoning_twice() {
        with_level(ContextMetricsLevel::Detailed, collect(async {
            { let mut r=RequestTrace::begin("safe-model",&[],None,false);r.usage(&json!({"input_tokens":100,"output_tokens":20,"input_tokens_details":{"cached_tokens":80},"output_tokens_details":{"reasoning_tokens":12}}));r.complete(); }
            {let _unknown=RequestTrace::begin("safe-model",&[],None,true);}
            let s=snapshot().unwrap();assert_eq!(s["provider_input_tokens"],100);assert_eq!(s["provider_output_tokens"],20);
            assert_eq!(s["requests_missing_input_usage"],1);assert_eq!(s["failed_or_cancelled_requests"],1);
            assert!(s["recent_requests"][1]["input_tokens"].is_null());
        })).await;
        with_level(
            ContextMetricsLevel::Off,
            collect(async {
                let _request = RequestTrace::begin("safe-model", &[], None, false);
                assert!(snapshot().is_none());
            }),
        )
        .await;
    }
}
