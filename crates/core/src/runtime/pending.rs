//! Durable suspended jobs. Explicit commands resume saved input, never new input.
use super::*;

#[derive(serde::Serialize, serde::Deserialize)]
struct Pending {
    version: u32,
    job_id: String,
    messages: Vec<ChatMessage>,
}

pub(crate) fn command(text: &str) -> anyhow::Result<Option<Option<&str>>> {
    if text.split_whitespace().next() != Some("/retry-compaction") {
        return Ok(None);
    }
    let words = text.split_whitespace().collect::<Vec<_>>();
    anyhow::ensure!(words.len() <= 2 && !text.trim().contains('\n'), "/retry-compaction accepts only an optional job ID; additional instructions must not be discarded");
    let id = words.get(1).copied();
    anyhow::ensure!(id.is_none_or(valid_id), "Invalid pending compaction job ID");
    Ok(Some(id))
}
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
/// Publish the stable ID first so it can be resumed independently of the latest pointer.
pub(super) fn save_job(
    directory: &std::path::Path,
    messages: &[ChatMessage],
) -> anyhow::Result<Vec<u8>> {
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
    )?;
    Ok(bytes)
}

pub(super) fn save_latest(directory: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    publish(&directory.join("pending-compaction.json"), bytes)
}

#[cfg(test)]
fn save(directory: &std::path::Path, messages: &[ChatMessage]) -> anyhow::Result<()> {
    let bytes = save_job(directory, messages)?;
    save_latest(directory, &bytes)
}

pub(super) fn restore(
    directory: &std::path::Path,
    system: ChatMessage,
    selected: Option<&str>,
) -> anyhow::Result<Vec<ChatMessage>> {
    anyhow::ensure!(
        selected.is_none_or(valid_id),
        "Invalid pending compaction job ID"
    );
    let path = directory.join(selected.map_or_else(
        || "pending-compaction.json".into(),
        |id| format!("pending-job-{id}.json"),
    ));
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
    let pointer = directory.join("pending-compaction.json");
    let active = std::fs::read(&pointer)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Pending>(&bytes).ok());
    if active.is_some_and(|active| active.job_id == job) {
        std::fs::remove_file(pointer)?;
    }
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
        save(directory.path(), &first).unwrap();
        let first_id = id(&first).unwrap();
        save(directory.path(), &second).unwrap();
        complete(directory.path(), &id(&second).unwrap()).unwrap();
        let restored = restore(directory.path(), first[0].clone(), Some(&first_id)).unwrap();
        assert_eq!(
            serde_json::to_value(restored).unwrap(),
            serde_json::to_value(first).unwrap()
        );
        assert!(command("/retry-compaction\nDo not edit").is_err());
        assert!(command("/retry-compaction ../../../outside").is_err());
        assert_eq!(
            command(&format!("/retry-compaction {first_id}")).unwrap(),
            Some(Some(first_id.as_str()))
        );
    }
}
