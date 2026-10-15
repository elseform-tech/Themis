//! Internal durable document-preparation job inputs; user messages never become
//! compaction commands. Context checkpoints need none: their saved snapshot is the input.

fn publish(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    let temporary = path.with_extension(format!(
        "{}.tmp",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    let result = (|| {
        std::io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

pub(super) fn save_document(
    directory: &std::path::Path,
    job: &str,
    capsule: &super::documents::DocumentCapsule,
    budget: usize,
    allowance: u32,
    config: &crate::configuration::CompactionConfig,
) -> anyhow::Result<()> {
    publish(
        &directory.join(format!("pending-job-{job}.json")),
        &serde_json::to_vec(
            &serde_json::json!({"version":1,"purpose":"document_preparation","job_id":job,"source":capsule,"effective_context_budget":budget,"output_allowance":allowance,"captured_configuration":config,"summary_pipeline":super::documents::PIPELINE_VERSION}),
        )?,
    )
}

pub(super) fn complete(directory: &std::path::Path, job: &str) -> anyhow::Result<()> {
    match std::fs::remove_file(directory.join(format!("pending-job-{job}.json"))) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
