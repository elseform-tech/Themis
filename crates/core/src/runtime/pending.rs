//! Internal durable job inputs; user messages never become compaction commands.
use super::*;

#[derive(serde::Serialize, serde::Deserialize)]
struct Pending {
    version: u32,
    job_id: String,
    messages: Vec<ChatMessage>,
}

#[cfg(test)]
fn valid_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn id(messages: &[ChatMessage]) -> anyhow::Result<String> {
    let messages = messages
        .iter()
        .filter(|m| m.role != ChatRole::System)
        .collect::<Vec<_>>();
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(&messages)?.hash(&mut hash);
    Ok(format!("{:016x}", hash.finish()))
}
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
/// Save exact job inputs under a stable ID; no latest-job pointer is required.
pub(super) fn save_job(
    directory: &std::path::Path,
    messages: &[ChatMessage],
) -> anyhow::Result<()> {
    let job_id = id(messages)?;
    let bytes = serde_json::to_vec(&Pending {
        version: 1,
        job_id: job_id.clone(),
        messages: messages
            .iter()
            .filter(|m| m.role != ChatRole::System)
            .cloned()
            .collect(),
    })?;
    publish(
        &directory.join(format!("pending-job-{job_id}.json")),
        &bytes,
    )
}

#[cfg(test)]
pub(super) fn restore(
    directory: &std::path::Path,
    system: ChatMessage,
    selected: &str,
) -> anyhow::Result<Vec<ChatMessage>> {
    anyhow::ensure!(valid_id(selected), "Invalid pending compaction job ID");
    let path = directory.join(format!("pending-job-{selected}.json"));
    anyhow::ensure!(
        std::fs::symlink_metadata(&path)?.is_file()
            && !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
        "pending job must be a regular local file"
    );
    let pending: Pending = serde_json::from_slice(&std::fs::read(path)?)?;
    anyhow::ensure!(
        pending.version == 1
            && !pending.messages.is_empty()
            && pending.messages.iter().all(|m| m.role != ChatRole::System)
            && pending.job_id == id(&pending.messages)?,
        "invalid pending compaction job"
    );
    let mut messages = vec![system];
    messages.extend(pending.messages);
    Ok(messages)
}

pub(super) fn complete(directory: &std::path::Path, job: &str) -> anyhow::Result<()> {
    match std::fs::remove_file(directory.join(format!("pending-job-{job}.json"))) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unrelated_jobs_do_not_destroy_an_older_suspended_task() {
        let directory = tempfile::tempdir().unwrap();
        let first = initial_messages(Vec::new(), "Task A".into());
        let second = initial_messages(Vec::new(), "Task B".into());
        save_job(directory.path(), &first).unwrap();
        let first_id = id(&first).unwrap();
        save_job(directory.path(), &second).unwrap();
        complete(directory.path(), &id(&second).unwrap()).unwrap();
        let restored = restore(directory.path(), first[0].clone(), &first_id).unwrap();
        assert_eq!(
            serde_json::to_value(restored).unwrap(),
            serde_json::to_value(first).unwrap()
        );
    }
}
