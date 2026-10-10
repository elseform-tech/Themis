//! Sequential, resumable summaries. Checkpoints are installed only by the caller.
use super::*;

const VERSION: &str = "adaptive-context-v1";

fn fingerprint(bytes: &[u8]) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    format!("{:016x}", hash.finish())
}

fn fragments(messages: &[ChatMessage], bytes: usize) -> anyhow::Result<Vec<String>> {
    let mut fragments = Vec::new();
    let mut packet = String::new();
    anyhow::ensure!(bytes >= 512, "source packet budget is too small");
    for (index, (record, message)) in evidence_records(messages)?
        .into_iter()
        .zip(
            messages
                .iter()
                .filter(|message| message.role != ChatRole::System),
        )
        .enumerate()
    {
        if message.role == ChatRole::Assistant
            && message.content.starts_with("Earlier context checkpoint")
        {
            continue;
        }
        let mut value: serde_json::Value = serde_json::from_str(&record)?;
        if value["evidence_kind"] == "recovery" {
            continue;
        }
        if let Some(results) = value["message_type"]
            .get_mut("ToolResult")
            .and_then(serde_json::Value::as_array_mut)
        {
            results.retain(|result| result["evidence_kind"] != "recovery");
        }
        let record = value.to_string();
        let mut offset = 0;
        while offset < record.len() {
            let mut end = (offset + bytes.saturating_sub(256) / 2).min(record.len());
            while !record.is_char_boundary(end) {
                end -= 1;
            }
            let fragment = serde_json::json!({"source_record": index+1, "serialized_byte_start":offset,
                "serialized_byte_end":end, "serialized_total_bytes":record.len(), "original_record_fragment": &record[offset..end]}).to_string();
            anyhow::ensure!(
                fragment.len() <= bytes,
                "encoded source fragment exceeds budget"
            );
            if !packet.is_empty() && packet.len() + 1 + fragment.len() > bytes {
                fragments.push(std::mem::take(&mut packet));
            }
            if !packet.is_empty() {
                packet.push('\n');
            }
            packet.push_str(&fragment);
            offset = end;
        }
    }
    if !packet.is_empty() {
        fragments.push(packet);
    }
    Ok(fragments)
}

fn stage_time_slice(
    remaining: std::time::Duration,
    remaining_stages: usize,
) -> std::time::Duration {
    if remaining_stages <= 1 {
        return remaining;
    }
    // Reserve a consolidation opportunity (at most a fifth of remaining time)
    // and five seconds per downstream source stage, not every possible retry.
    let reserve = (remaining / 5).min(std::time::Duration::from_secs(60))
        + std::time::Duration::from_secs(5 * remaining_stages.saturating_sub(2) as u64);
    remaining.saturating_sub(reserve.min(remaining / 2))
}

fn cache_directory(cache: &std::path::Path) -> anyhow::Result<()> {
    let parent = cache
        .parent()
        .ok_or_else(|| anyhow!("missing summary cache parent"))?
        .canonicalize()?;
    match std::fs::create_dir(cache) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = std::fs::symlink_metadata(cache)?;
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Summary cache must be a contained non-symlink directory"
    );
    anyhow::ensure!(
        cache.canonicalize()?
            == parent.join(
                cache
                    .file_name()
                    .ok_or_else(|| anyhow!("missing summary cache name"))?
            ),
        "Summary cache escapes its parent"
    );
    Ok(())
}

async fn stage(
    llm: &Arc<dyn LLMProvider>,
    request: &[ChatMessage],
    allowance: u32,
    cache: &std::path::Path,
    job: &str,
    index: usize,
    remaining_stages: usize,
) -> anyhow::Result<String> {
    cache_directory(cache)?;
    let input = fingerprint(&serde_json::to_vec(request)?);
    let path = cache.join(format!("{job}-{index}-{input}.json"));
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            // Accepted decoded text is at most allowance * 8 bytes; JSON control escapes can expand sixfold.
            let limit = allowance as u64 * 48 + 4096;
            anyhow::ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= limit,
                "Invalid or oversized saved summary stage"
            );
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(
                &mut std::io::Read::take(std::fs::File::open(&path)?, limit + 1),
                &mut bytes,
            )?;
            anyhow::ensure!(
                bytes.len() as u64 <= limit,
                "Saved summary stage grew beyond its bound"
            );
            let saved: serde_json::Value = serde_json::from_slice(&bytes)?;
            let text = saved["summary"]
                .as_str()
                .ok_or_else(|| anyhow!("Saved summary stage has no text"))?;
            anyhow::ensure!(
                saved["input"] == input
                    && saved["pipeline"] == VERSION
                    && text.len().div_ceil(4) <= allowance as usize * 2,
                "Saved summary stage does not match its bounded input"
            );
            validate_checkpoint(text)?;
            crate::diagnostics::emit(
                "compaction",
                "stage_reused",
                "info",
                serde_json::json!({"job_id":job,"stage":index}),
            );
            return Ok(text.to_owned());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    crate::diagnostics::emit(
        "compaction",
        "stage_started",
        "info",
        serde_json::json!({"job_id":job,"stage":index,"estimated_input_tokens":estimated_tokens(request),"output_limit_tokens":allowance}),
    );
    let remaining = SUMMARY_DEADLINE
        .try_with(|deadline| deadline.saturating_duration_since(tokio::time::Instant::now()))
        .unwrap_or(std::time::Duration::from_secs(
            compaction_config().timeout_seconds,
        ));
    let slice = stage_time_slice(remaining, remaining_stages);
    let deadline = tokio::time::Instant::now() + slice;
    let summary = tokio::time::timeout_at(
        deadline,
        SUMMARY_STAGE_DEADLINE.scope(
            deadline,
            crate::context_metrics::with_stage(
                job,
                index,
                compaction_chat(llm, request, allowance),
            ),
        ),
    )
    .await
    .map_err(|_| anyhow!("Required summary stage timed out"))??;
    anyhow::ensure!(
        summary.len().div_ceil(4) <= allowance as usize * 2,
        "summary stage exceeded its bounded allowance"
    );
    cache_directory(cache)?;
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let bytes = serde_json::to_vec(
        &serde_json::json!({"pipeline":VERSION,"input":input,"summary":summary}),
    )?;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    let written = std::io::Write::write_all(&mut file, &bytes).and_then(|_| file.sync_all());
    drop(file);
    let published = written.and_then(|_| std::fs::hard_link(&temporary, &path));
    let _ = std::fs::remove_file(temporary);
    published?;
    crate::diagnostics::emit(
        "compaction",
        "stage_completed",
        "info",
        serde_json::json!({"job_id":job,"stage":index,"summary_bytes":summary.len()}),
    );
    Ok(summary)
}

pub(super) async fn summarize(
    llm: &Arc<dyn LLMProvider>,
    messages: &[ChatMessage],
    current: &str,
    budget: usize,
    allowance: u32,
    evidence: &std::path::Path,
) -> anyhow::Result<String> {
    let config = compaction_config();
    let previous = prior_checkpoint(messages)
        .map(checkpoint_task_state)
        .unwrap_or("");
    let fixed = estimated_tokens(&context_update_messages("", previous, current, true));
    let available = budget.saturating_sub(fixed + allowance as usize * 2 + 2048);
    anyhow::ensure!(
        available >= 1024,
        "summary instructions leave no room for original records"
    );
    // A JSON string may double its serialized size. The margin bounds both encodings.
    let fragments = fragments(messages, available * 2)?;
    anyhow::ensure!(!fragments.is_empty() && fragments.len() < config.max_stages,
        "summary requires {} source stages plus consolidation; configured maximum is {}. Originals preserved", fragments.len(), config.max_stages);
    let job = fingerprint(&serde_json::to_vec(&(
        VERSION,
        llm.model(),
        &config,
        messages,
        current,
    ))?);
    let cache = evidence
        .parent()
        .ok_or_else(|| anyhow!("missing evidence directory"))?
        .join("summary-jobs");
    // Reserve the complete consolidation packet before any stage is dispatched.
    let stage_allowance = allowance.min((available / (fragments.len() * 3)).max(128) as u32);
    let mut notes = Vec::new();
    for (index, fragment) in fragments.iter().enumerate() {
        let request = context_update_messages(fragment, previous, current, true);
        anyhow::ensure!(
            estimated_tokens(&request) + stage_allowance as usize * 2 + 1024 <= budget,
            "source stage exceeds request budget"
        );
        let summary = stage(
            llm,
            &request,
            stage_allowance,
            &cache,
            &job,
            index,
            fragments.len() + 1 - index,
        )
        .await?;
        notes.push(format!(
            "SOURCE STAGE {} OF {} (lossy notes, reopen originals for quotes):\n{}",
            index + 1,
            fragments.len(),
            summary
        ));
    }
    let request = context_update_messages(&notes.join("\n"), previous, current, false);
    anyhow::ensure!(
        estimated_tokens(&request) + allowance as usize * 2 + 1024 <= budget,
        "consolidated stage notes exceed budget; originals and completed stages preserved"
    );
    let mut summary = stage(llm, &request, allowance, &cache, &job, fragments.len(), 1).await?;
    summary.push_str(&format!("\n\nComplete source coverage: {} ordered source fragments supplied to completed stages; consolidation completed. Summaries remain lossy; this is processing coverage, not a guarantee that every fact was retained. Exact quotations require original records.", fragments.len()));
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn stage_cache_rejects_symlink_escape_and_cannot_truncate_originals() {
        use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        // Accepted decoded text can expand sixfold when cached as JSON.
        let summary = format!("Goal/constraints: preserve. Confirmed findings: source supplied. Completed changes/tests: none. Unresolved work: continue. Next action: answer.{}", "\u{0001}".repeat(1700));
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":summary},"finish_reason":"stop"}]}))).mount(&server).await;
        let llm = crate::providers::resolve(
            &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "test-key")
                .with_model("test-model")
                .with_base_url(server.uri()),
        )
        .await
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let alias = root.path().join("summary-jobs");
        std::os::unix::fs::symlink(outside.path(), &alias).unwrap();
        let request = context_update_messages("original source", "", "continue", true);
        let result = stage(&llm, &request, 256, &alias, "test", 0, 1).await;
        assert!(
            result.is_err(),
            "cache directory symlinks must be rejected before reading or publishing"
        );
        assert!(server.received_requests().await.unwrap().is_empty());
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
        std::fs::remove_file(&alias).unwrap();
        std::fs::create_dir(&alias).unwrap();
        let input = fingerprint(&serde_json::to_vec(&request).unwrap());
        let cache = alias.join(format!("test-0-{input}.json"));
        let original = root.path().join("snapshot-original.jsonl");
        std::fs::write(&original, "immutable original").unwrap();
        std::fs::hard_link(&original, cache.with_extension("tmp")).unwrap();
        stage(&llm, &request, 256, &alias, "test", 0, 1)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(original).unwrap(),
            "immutable original"
        );
        assert!(cache.is_file());
        assert!(std::fs::metadata(&cache).unwrap().len() > 256 * 16 + 4096);
        assert_eq!(
            stage(&llm, &request, 256, &alias, "test", 0, 1)
                .await
                .unwrap(),
            summary
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn stage_deadline_includes_retry_backoff() {
        use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "60"))
            .mount(&server)
            .await;
        let llm = crate::providers::resolve(
            &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "test-key")
                .with_model("test-model")
                .with_base_url(server.uri()),
        )
        .await
        .unwrap();
        let request = context_update_messages("original state", "", "continue", true);
        let directory = tempfile::tempdir().unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(400),
            SUMMARY_DEADLINE.scope(
                tokio::time::Instant::now() + std::time::Duration::from_millis(100),
                stage(&llm, &request, 256, directory.path(), "test", 0, 2),
            ),
        )
        .await;
        assert!(
            result.is_ok(),
            "backoff must not outlive the stage deadline"
        );
        assert!(result
            .unwrap()
            .unwrap_err()
            .to_string()
            // The request-loop and stage guards share a deadline; either can fire first.
            .contains("timed out"));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn deadline_does_not_reserve_every_hypothetical_retry() {
        let slice = stage_time_slice(std::time::Duration::from_secs(300), 11);
        assert_eq!(slice, std::time::Duration::from_secs(195));
        assert_eq!(
            stage_time_slice(std::time::Duration::from_secs(300), 1),
            std::time::Duration::from_secs(300)
        );
        assert!(
            stage_time_slice(std::time::Duration::from_secs(2), 10)
                >= std::time::Duration::from_secs(1)
        );
    }

    #[test]
    fn packs_short_records_without_losing_source_coordinates() {
        let messages = (0..40)
            .map(|index| ChatMessage {
                role: ChatRole::User,
                message_type: MessageType::Text,
                content: format!("record-{index}: {}", "x".repeat(1800)),
            })
            .collect::<Vec<_>>();
        let packets = fragments(&messages, 12_000).unwrap();
        assert!(packets.len() < 12, "short records must share source stages");
        let joined = packets.join("\n");
        for index in 0..40 {
            assert!(joined.contains(&format!("record-{index}:")));
        }
        assert!(packets.iter().all(|packet| packet.len() <= 12_000));
    }
}
