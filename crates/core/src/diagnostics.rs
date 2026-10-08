//! Bounded local diagnostics, separate from conversation contents and credentials.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const MAX_RECORD: usize = 16 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticRecord {
    pub timestamp: u64,
    pub level: String,
    pub service: String,
    pub event: String,
    pub details: Value,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticFilter {
    pub service: Option<String>,
    pub level: Option<String>,
    pub operation_id: Option<String>,
    pub limit: Option<usize>,
}

pub struct DiagnosticLog {
    directory: PathBuf,
    lock: Mutex<()>,
    level: std::sync::atomic::AtomicU8,
}

impl DiagnosticLog {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            lock: Mutex::new(()),
            level: std::sync::atomic::AtomicU8::new(1),
        }
    }

    fn path(&self, index: usize) -> PathBuf {
        self.directory.join(format!("diagnostics-{index}.jsonl"))
    }

    pub fn set_level(&self, level: &str) {
        self.level
            .store(severity(level), std::sync::atomic::Ordering::Relaxed);
    }

    /// Callers supply metadata, never prompts, raw provider errors or request bodies.
    pub fn append(
        &self,
        service: &str,
        event: &str,
        level: &str,
        details: Value,
    ) -> std::io::Result<()> {
        self.append_with_level(service, event, level, details, None)
    }

    pub fn append_with_level(
        &self,
        service: &str,
        event: &str,
        level: &str,
        details: Value,
        minimum: Option<&str>,
    ) -> std::io::Result<()> {
        let minimum = minimum.map_or_else(
            || self.level.load(std::sync::atomic::Ordering::Relaxed),
            severity,
        );
        if severity(level) < minimum {
            return Ok(());
        }
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        std::fs::create_dir_all(&self.directory)?;
        let record = DiagnosticRecord {
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            level: level.into(),
            service: service.into(),
            event: event.into(),
            details: redact(details),
        };
        let mut bytes = serde_json::to_vec(&record)?;
        if bytes.len() > MAX_RECORD {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Diagnostic metadata exceeds limit",
            ));
        }
        bytes.push(b'\n');
        if std::fs::metadata(self.path(0)).is_ok_and(|m| m.len() + bytes.len() as u64 > MAX_BYTES) {
            if self.path(2).exists() {
                std::fs::remove_file(self.path(2))?;
            }
            if self.path(1).exists() {
                std::fs::rename(self.path(1), self.path(2))?;
            }
            std::fs::rename(self.path(0), self.path(1))?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(self.path(0))?.write_all(&bytes)
    }

    pub fn query(&self, filter: &DiagnosticFilter) -> std::io::Result<Vec<DiagnosticRecord>> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut records = std::collections::VecDeque::new();
        let limit = filter.limit.unwrap_or(200).clamp(1, 2000);
        for index in (0..=2).rev() {
            let path = self.path(index);
            if !path.exists() {
                continue;
            }
            for line in std::io::BufReader::new(std::fs::File::open(path)?).lines() {
                let line = line?;
                let Ok(record) = serde_json::from_str::<DiagnosticRecord>(&line) else {
                    continue;
                };
                if filter
                    .service
                    .as_ref()
                    .is_some_and(|v| v != &record.service)
                    || filter.level.as_ref().is_some_and(|v| v != &record.level)
                    || filter
                        .operation_id
                        .as_ref()
                        .is_some_and(|v| record.details["operation_id"].as_str() != Some(v))
                {
                    continue;
                }
                records.push_back(record);
                if records.len() > limit {
                    records.pop_front();
                }
            }
        }
        Ok(records.into_iter().collect())
    }
}

fn severity(level: &str) -> u8 {
    match level {
        "debug" => 0,
        "info" => 1,
        "warn" => 2,
        _ => 3,
    }
}

fn redact(value: Value) -> Value {
    match value {
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| {
                    let normalized = key.to_ascii_lowercase();
                    let sensitive = [
                        "secret",
                        "password",
                        "authorization",
                        "api_key",
                        "apikey",
                        "token",
                        "credential",
                        "content",
                        "prompt",
                        "arguments",
                        "output",
                        "command",
                    ]
                    .iter()
                    .any(|word| normalized.contains(word));
                    let value = if sensitive
                        && !normalized.ends_with("_count")
                        && !normalized.ends_with("_tokens")
                        && !normalized.ends_with("_bytes")
                    {
                        Value::String("[redacted]".into())
                    } else {
                        redact(value)
                    };
                    (key, value)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(redact).collect()),
        other => other,
    }
}

#[derive(Clone)]
pub struct DiagnosticContext {
    pub log: Arc<DiagnosticLog>,
    pub thread_id: String,
    pub run_id: String,
    pub level: String,
}
tokio::task_local! { pub static ACTIVE: DiagnosticContext; }

pub fn emit(service: &str, event: &str, level: &str, mut details: Value) {
    let _ = ACTIVE.try_with(|context| {
        details["thread_id"] = context.thread_id.clone().into();
        details["run_id"] = context.run_id.clone().into();
        if details.get("operation_id").is_none() {
            details["operation_id"] = context.run_id.clone().into();
        }
        context
            .log
            .append_with_level(service, event, level, details, Some(&context.level))
    });
}

/// Drain child stderr continuously so a noisy process cannot block on its pipe.
/// Retain at most 4 KiB, only at debug level, masking known environment secrets.
pub fn capture_stderr<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    mut stderr: R,
    service: &'static str,
    sensitive_values: Vec<String>,
) {
    use tokio::io::AsyncReadExt;
    let context = ACTIVE.try_with(Clone::clone).ok();
    let mut secrets = std::env::vars()
        .filter(|(name, value)| {
            value.len() >= 4
                && ["TOKEN", "KEY", "PASSWORD", "SECRET", "AUTH"]
                    .iter()
                    .any(|word| name.to_ascii_uppercase().contains(word))
        })
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    secrets.extend(
        sensitive_values
            .into_iter()
            .filter(|value| !value.is_empty()),
    );
    tokio::spawn(async move {
        let mut bytes = [0u8; 1024];
        let mut retained = Vec::new();
        let mut total = 0u64;
        loop {
            let count = match stderr.read(&mut bytes).await {
                Ok(0) | Err(_) => break,
                Ok(count) => count,
            };
            if total == 0 {
                if let Some(context) = &context {
                    let _ = context.log.append_with_level(
                        service,
                        "stderr_observed",
                        "warn",
                        serde_json::json!({"run_id":context.run_id,"thread_id":context.thread_id,"operation_id":context.run_id}),
                        Some(&context.level),
                    );
                }
            }
            total += count as u64;
            retained.extend_from_slice(&bytes[..count.min(4096 - retained.len())]);
        }
        if let Some(context) = context.filter(|_| total > 0) {
            // Drop the last incomplete line at the cap so a truncated secret cannot leak.
            if retained.len() == 4096 {
                retained.truncate(retained.iter().rposition(|b| *b == b'\n').unwrap_or(0));
            }
            let text = redact_stderr(&retained, &secrets);
            let _=context.log.append_with_level(service,"stderr_excerpt","debug",serde_json::json!({"run_id":context.run_id,"thread_id":context.thread_id,"operation_id":context.run_id,"stderr":text,"stderr_bytes":total}),Some(&context.level));
        }
    });
}

fn redact_stderr(retained: &[u8], secrets: &[String]) -> String {
    let mut text = String::from_utf8_lossy(retained).into_owned();
    // Mask before normalizing lines, including retained fragments of multiline credentials.
    let mut fragments = secrets
        .iter()
        .flat_map(|secret| std::iter::once(secret.as_str()).chain(secret.lines()))
        .filter(|secret| !secret.is_empty())
        .collect::<Vec<_>>();
    fragments.sort_unstable_by_key(|secret| std::cmp::Reverse(secret.len()));
    for secret in fragments {
        text = text.replace(secret, "[redacted]");
    }
    text.lines()
        .map(|line| {
            if [
                "token",
                "password",
                "secret",
                "authorization",
                "api_key",
                "apikey",
                "private key",
                "bearer",
                "prompt",
                "content",
                "arguments",
                "output",
                "command",
            ]
            .iter()
            .any(|word| line.to_ascii_lowercase().contains(word))
            {
                "[redacted]".to_owned()
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Never accept a user-controlled path as a diagnostic output location.
pub fn directory_for(app_directory: &Path) -> PathBuf {
    app_directory.join("logs")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_run_level_is_independent_of_global_level() {
        let dir = tempfile::tempdir().unwrap();
        let log = DiagnosticLog::new(dir.path().into());
        log.set_level("error");
        log.append("server", "filtered", "info", serde_json::json!({}))
            .unwrap();
        log.append_with_level(
            "mcp",
            "run_debug",
            "debug",
            serde_json::json!({}),
            Some("debug"),
        )
        .unwrap();
        log.append_with_level(
            "mcp",
            "run_filtered",
            "info",
            serde_json::json!({}),
            Some("warn"),
        )
        .unwrap();
        let records = log.query(&DiagnosticFilter::default()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].event, "run_debug");
    }
    #[test]
    fn stderr_masks_retained_multiline_credential_fragments() {
        let output = redact_stderr(
            b"startup\nfirst-fragment\n",
            &["first-fragment\nsecond-fragment\n".into()],
        );
        assert!(!output.contains("first-fragment"));
        assert!(output.contains("startup"));
        assert_eq!(
            redact_stderr(br#"{"arguments":{"content":"private-marker"}}"#, &[]),
            "[redacted]"
        );
        assert_eq!(
            redact_stderr(b"abc-longer", &["abc".into(), "abc-longer".into()]),
            "[redacted]"
        );
    }

    #[tokio::test]
    async fn stderr_redacts_explicit_arbitrary_named_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(DiagnosticLog::new(dir.path().into()));
        let context = DiagnosticContext {
            log: log.clone(),
            thread_id: "thread".into(),
            run_id: "run".into(),
            level: "debug".into(),
        };
        let (mut writer, reader) = tokio::io::duplex(1024);
        ACTIVE
            .scope(context, async {
                capture_stderr(reader, "mcp", vec!["synthetic-access-value".into()]);
            })
            .await;
        use tokio::io::AsyncWriteExt;
        writer
            .write_all(b"Connection refused\nDOCS_ACCESS=synthetic-access-value\n")
            .await
            .unwrap();
        drop(writer);
        let records = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let records = log.query(&DiagnosticFilter::default()).unwrap();
                if records.iter().any(|r| r.event == "stderr_excerpt") {
                    break records;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let text = serde_json::to_string(&records).unwrap();
        assert!(!text.contains("synthetic-access-value"));
        assert!(text.contains("Connection refused"));
        assert!(records.iter().all(|r| r.details["operation_id"] == "run"));
    }

    #[test]
    fn persists_filters_redacts_and_rotates() {
        let dir = tempfile::tempdir().unwrap();
        let log = DiagnosticLog::new(dir.path().into());
        log.append(
            "mcp",
            "import_failed",
            "error",
            serde_json::json!({"operation_id":"a", "api_key":"private", "input_tokens":42}),
        )
        .unwrap();
        let fresh = DiagnosticLog::new(dir.path().into());
        let records = fresh
            .query(&DiagnosticFilter {
                operation_id: Some("a".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].details["api_key"], "[redacted]");
        assert_eq!(records[0].details["input_tokens"], 42);
        std::fs::write(log.path(0), vec![b' '; MAX_BYTES as usize]).unwrap();
        log.append("server", "started", "info", serde_json::json!({}))
            .unwrap();
        assert!(log.path(1).exists());
        assert_eq!(log.query(&DiagnosticFilter::default()).unwrap().len(), 1);
    }
}
