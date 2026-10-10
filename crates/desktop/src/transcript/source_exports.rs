//! Incremental immutable exports make historical original exchanges tool-readable.
use super::*;
use std::io::Write;

impl TranscriptStore {
    pub fn publish_context_sources(&self, thread: &str, directory: &Path) -> Result<(), String> {
        let directory = directory.canonicalize().map_err(|e| e.to_string())?;
        let key = directory.to_string_lossy();
        let (rows, last) = {
            let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
            let previous:Option<(i64,String)> = connection.query_row("SELECT last_seq,last_file FROM context_source_exports WHERE thread_id=?1 AND directory=?2",params![thread,key],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|e|e.to_string())?;
            let last = previous
                .filter(|(_, file)| {
                    let path = directory.join(file);
                    std::fs::symlink_metadata(path)
                        .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
                })
                .map_or(0, |p| p.0);
            let mut statement=connection.prepare("SELECT seq,role,text FROM context_turns WHERE thread_id=?1 AND seq>?2 ORDER BY seq").map_err(|e|e.to_string())?;
            let rows = statement
                .query_map(params![thread, last], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            (rows, last)
        };
        let Some((end, _, _)) = rows.last() else {
            return Ok(());
        };
        let filename = format!(
            "history-source-{}-{end}-{}.jsonl",
            last + 1,
            uuid::Uuid::new_v4()
        );
        let final_path = directory.join(&filename);
        let temporary = final_path.with_extension("tmp");
        let result = (|| -> std::io::Result<()> {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            let mut writer = std::io::BufWriter::new(file);
            for (seq, role, text) in &rows {
                serde_json::to_writer(
                    &mut writer,
                    &serde_json::json!({"source_seq":seq,"role":if role=="user" {"User"} else {"Assistant"},"message_type":"Text","content":text,"evidence_kind":"original_exchange"}),
                )?;
                writer.write_all(b"\n")?;
            }
            writer.flush()?;
            writer.get_ref().sync_all()?;
            std::fs::rename(&temporary, &final_path)
        })();
        if let Err(error) = result {
            let _ = std::fs::remove_file(&temporary);
            return Err(format!("save historical originals: {error}"));
        }
        let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        connection.execute("INSERT INTO context_source_exports(thread_id,directory,last_seq,last_file) VALUES(?1,?2,?3,?4) ON CONFLICT(thread_id,directory) DO UPDATE SET last_seq=excluded.last_seq,last_file=excluded.last_file WHERE excluded.last_seq>=context_source_exports.last_seq",params![thread,key,end,filename]).map_err(|e|e.to_string())?;
        Ok(())
    }
}
