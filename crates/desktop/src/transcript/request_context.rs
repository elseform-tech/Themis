//! The conversation a run is given: saved turns, checkpoints and recall of earlier requests.

use super::*;

impl TranscriptStore {
    pub fn context(
        &self,
        thread_id: &str,
        max_messages: usize,
    ) -> Result<Vec<ConversationTurn>, String> {
        let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        let checkpoint: Option<(i64, String)> = connection
            .query_row(
                "SELECT through_seq,summary FROM context_checkpoints WHERE thread_id=?1",
                [thread_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("read context checkpoint: {e}"))?;
        let through_seq = checkpoint.as_ref().map_or(0, |checkpoint| checkpoint.0);
        let mut statement = connection
            .prepare(
                "SELECT role,text FROM context_turns WHERE thread_id=?1 AND seq>?2 ORDER BY seq",
            )
            .map_err(|e| format!("read context: {e}"))?;
        let rows = statement
            .query_map(params![thread_id, through_seq], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("read context: {e}"))?;
        let mut turns = Vec::new();
        if let Some((_, summary)) = checkpoint {
            let first: Option<(i64, String)> = connection
                .query_row(
                    "SELECT seq,text FROM context_turns WHERE thread_id=?1 AND seq<=?2 AND role='user' ORDER BY seq LIMIT 1",
                    params![thread_id, through_seq],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| format!("read first user request: {e}"))?;
            let mut statement = connection
                .prepare("SELECT seq,text FROM context_turns WHERE thread_id=?1 AND seq<=?2 AND role='user' ORDER BY seq DESC LIMIT 2")
                .map_err(|e| format!("read checkpoint user requests: {e}"))?;
            let mut requests: Vec<(i64, String)> = statement
                .query_map(params![thread_id, through_seq], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .map_err(|e| format!("read checkpoint user requests: {e}"))?
                .collect::<Result<_, _>>()
                .map_err(|e| format!("read checkpoint user requests: {e}"))?;
            requests.extend(first);
            requests.sort_by_key(|request| request.0);
            requests.dedup_by_key(|request| request.0);
            let (_, recent) = checkpoint_parts(&summary);
            requests.retain(|(_, text)| {
                !recent
                    .iter()
                    .any(|message| message.role == ChatRole::User && &message.content == text)
            });
            turns.push(ConversationTurn {
                role: ConversationRole::Checkpoint,
                text: summary,
            });
            if !requests.is_empty() {
                turns.push(ConversationTurn {
                    role: ConversationRole::Assistant,
                    text: format!(
                        "Verbatim first and recent user requests (historical):\n{}",
                        requests
                            .into_iter()
                            .map(|request| request.1)
                            .collect::<Vec<_>>()
                            .join("\n")
                    ),
                });
            }
        }
        for row in rows {
            let (role, text) = row.map_err(|e| format!("read context: {e}"))?;
            turns.push(ConversationTurn {
                role: if role == "user" {
                    ConversationRole::User
                } else {
                    ConversationRole::Assistant
                },
                text,
            });
        }
        let skip = turns.len().saturating_sub(max_messages);
        Ok(turns.into_iter().skip(skip).collect())
    }

    /// The chat's first user message, which every context checkpoint request carries.
    pub fn first_user_request(&self, thread_id: &str) -> Result<Option<String>, String> {
        let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        connection
            .query_row(
                "SELECT text FROM context_turns WHERE thread_id=?1 AND role='user' ORDER BY seq LIMIT 1",
                [thread_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("read first user request: {e}"))
    }

    /// Select original exchanges for recall, keeping historical text out of the instruction role.
    pub fn context_for_request(
        &self,
        thread_id: &str,
        request: &str,
        max_bytes: usize,
    ) -> Result<Vec<ConversationTurn>, String> {
        let mut context = self.context(thread_id, usize::MAX)?;
        context.retain(|turn| {
            !turn
                .text
                .starts_with("Verbatim first and recent user requests (historical):")
        });
        // A greeting or thanks carries nothing to look up; history anchors would only
        // pull the previous task back into a conversation that has moved on.
        if themis_core::social::is_social_request(request) {
            return Ok(context);
        }
        let present = context
            .iter()
            .map(|turn| turn.text.clone())
            .collect::<Vec<_>>();
        let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        let allowance = (max_bytes / 3).min(16000);
        let mut records = Vec::<serde_json::Value>::new();
        let mut bytes = 0;
        let mut add = |seq: i64, role: String, offset: usize, text: String, purpose: &str| {
            let record = serde_json::json!({"source_seq":seq,"source_role":role,"purpose":purpose,
                "excerpt_char_offset":offset,"verbatim_excerpt":text,"decision_status":"unclassified; read the actual wording", "historical":true});
            let size = record.to_string().len();
            // Text already verbatim in the working context needs no second copy.
            // Short excerpts ("yes") must match a whole turn; substring matches would hide them.
            let duplicate = !text.trim().is_empty()
                && present.iter().any(|turn| {
                    if text.chars().count() >= MIN_SUBSTRING_DEDUPE_CHARS {
                        turn.contains(&text)
                    } else {
                        turn.trim() == text.trim()
                    }
                });
            if !duplicate
                && bytes + size <= allowance
                && !records.iter().any(|r| r["source_seq"] == seq)
            {
                bytes += size;
                records.push(record);
            }
        };
        // Exact latest exchange anchors vague follow-ups after a lossy checkpoint.
        let mut statement = connection.prepare("SELECT seq,role,substr(text,1,2400) FROM context_turns WHERE thread_id=?1 ORDER BY seq DESC LIMIT 4").map_err(|e|e.to_string())?;
        let latest = statement
            .query_map([thread_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in latest {
            let (seq, role, text) = row.map_err(|e| e.to_string())?;
            add(
                seq,
                role,
                0,
                text,
                "latest chronological exchange; current request chooses whether to resume it",
            );
        }
        let lower = request.to_lowercase();
        let mut terms = themis_core::social::content_terms(&lower);
        terms.truncate(12);
        terms.sort();
        terms.dedup();
        // Originals are searched locally; only bounded matching passages reach the model.
        let mut passage = connection.prepare("SELECT seq,role,max(1,instr(lower(text),?2)-700),substr(text,max(1,instr(lower(text),?2)-700),2400) FROM context_turns WHERE seq=?1").map_err(|e|e.to_string())?;
        for (term, seqs) in
            terms
                .iter()
                .zip(newest_turns_with_terms(&connection, thread_id, &terms)?)
        {
            for seq in seqs {
                let (seq, role, offset, text) = passage
                    .query_row(params![seq, term], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, usize>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    })
                    .map_err(|e| e.to_string())?;
                add(seq,role,offset-1,text,"lexical match in original exchange; relevance is not a decision or authorization");
            }
        }
        if lower.contains("topics") || lower.contains("have we discussed") {
            let mut statement = connection.prepare("SELECT seq,substr(text,1,240) FROM context_turns WHERE thread_id=?1 AND role='user' ORDER BY seq DESC LIMIT 60").map_err(|e|e.to_string())?;
            let rows = statement
                .query_map([thread_id], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (seq, text) = row.map_err(|e| e.to_string())?;
                add(
                    seq,
                    "user".into(),
                    0,
                    text,
                    "discussed-topic index; prefix only, not complete topic coverage",
                );
            }
        }
        records.sort_by_key(|r| r["source_seq"].as_i64().unwrap_or_default());
        if !records.is_empty() {
            context.push(ConversationTurn {
                role: ConversationRole::Assistant,
                text: format!(
                    "{}\n{}",
                    themis_core::prompts::DESKTOP_EXCHANGE_ANCHORS,
                    records
                        .iter()
                        .map(serde_json::Value::to_string)
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            });
        }
        Ok(context)
    }

    /// Latest aggregate for one run, without exposing other transcript payloads.
    pub fn context_usage(
        &self,
        thread_id: &str,
        run_id: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        let value:Option<String>=connection.query_row(
            "SELECT json_extract(item,'$.envelope.event.metrics') FROM history WHERE thread_id=?1 AND json_extract(item,'$.envelope.event.kind')='context_usage' AND (?2 IS NULL OR json_extract(item,'$.envelope.run_id')=?2) ORDER BY seq DESC LIMIT 1", params![thread_id,run_id],|row|row.get(0)).optional().map_err(|e|e.to_string())?;
        value
            .map(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
            .transpose()
            .map(|v| v.unwrap_or(serde_json::Value::Null))
    }
}

/// Most matching turns recalled for one term.
pub(super) const RECALL_MATCHES_PER_TERM: usize = 4;

/// For each term, the newest turns containing it, newest first, reading the chat once however
/// many terms there are and stopping when every term has all its matches. A match is what
/// `instr(lower(text),term)>0` finds, where SQLite's `lower` only folds ASCII.
pub(super) fn newest_turns_with_terms(
    connection: &Connection,
    thread_id: &str,
    terms: &[String],
) -> Result<Vec<Vec<i64>>, String> {
    let mut found = vec![Vec::new(); terms.len()];
    let mut statement = connection
        .prepare("SELECT seq,text FROM context_turns WHERE thread_id=?1 ORDER BY seq DESC")
        .map_err(|e| e.to_string())?;
    let mut rows = statement.query([thread_id]).map_err(|e| e.to_string())?;
    while found
        .iter()
        .any(|seqs| seqs.len() < RECALL_MATCHES_PER_TERM)
    {
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            break;
        };
        let seq: i64 = row.get(0).map_err(|e| e.to_string())?;
        let text = row
            .get_ref(1)
            .and_then(|text| Ok(text.as_bytes()?))
            .map_err(|e| e.to_string())?;
        let lowered = String::from_utf8_lossy(text).to_ascii_lowercase();
        for (term, seqs) in terms.iter().zip(&mut found) {
            if seqs.len() < RECALL_MATCHES_PER_TERM && lowered.contains(term.as_str()) {
                seqs.push(seq);
            }
        }
    }
    Ok(found)
}
