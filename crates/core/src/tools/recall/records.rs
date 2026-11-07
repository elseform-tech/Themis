//! Saved records: fields, envelopes and capped line reads.

use super::*;

/// One searchable text field of a saved record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordField {
    /// JSON pointer of the field in the record.
    pub pointer: String,
    /// Record role (`User`, `Assistant`, `Tool`).
    pub role: String,
    /// Tool name for a tool result.
    pub tool: Option<String>,
    /// `original`, `original_exchange`, `navigation` or `recovery`.
    pub evidence_kind: String,
    /// Field text; a tool result is decoded once (see `decoded`).
    pub text: String,
    /// True when `text` is the once-decoded tool envelope.
    pub decoded: bool,
}

/// Flattens a tool envelope the way `read_file` with `decode_tool_payload`
/// does: labeled leaf values, JSON decoded exactly once.
pub fn decode_envelope(text: &str) -> String {
    fn leaves(value: &Value, path: &str, output: &mut String) {
        match value {
            Value::Object(fields) => {
                for (key, value) in fields {
                    leaves(value, &format!("{path}/{key}"), output);
                }
            }
            Value::Array(items) => {
                for (index, value) in items.iter().enumerate() {
                    leaves(value, &format!("{path}/{index}"), output);
                }
            }
            _ => {
                output.push_str(&format!(
                    "{path}:\n{}\n",
                    value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_owned)
                ));
            }
        }
    }
    match serde_json::from_str::<Value>(text) {
        Ok(value) => {
            let mut output = String::new();
            leaves(&value, "", &mut output);
            output
        }
        Err(_) => text.to_owned(),
    }
}

/// Searchable fields of one JSONL record; empty for malformed lines.
///
/// Never panics on arbitrary bytes.
pub fn record_fields(line: &[u8]) -> Vec<RecordField> {
    let Ok(record) = serde_json::from_slice::<Value>(line) else {
        return Vec::new();
    };
    let role = record["role"].as_str().unwrap_or("").to_owned();
    let record_kind = match record["evidence_kind"].as_str() {
        Some(kind) => kind.to_owned(),
        None if role == "Assistant" => "navigation".to_owned(),
        None => "original".to_owned(),
    };
    let mut fields = Vec::new();
    if let Some(content) = record["content"].as_str().filter(|text| !text.is_empty()) {
        fields.push(RecordField {
            pointer: "/content".into(),
            role: role.clone(),
            tool: None,
            evidence_kind: record_kind.clone(),
            text: content.to_owned(),
            decoded: false,
        });
    }
    if let Some(results) = record["message_type"]["ToolResult"].as_array() {
        for (index, result) in results.iter().enumerate() {
            let Some(arguments) = result["function"]["arguments"]
                .as_str()
                .filter(|text| !text.is_empty())
            else {
                continue;
            };
            fields.push(RecordField {
                pointer: format!("/message_type/ToolResult/{index}/function/arguments"),
                role: role.clone(),
                tool: result["function"]["name"].as_str().map(str::to_owned),
                evidence_kind: result["evidence_kind"]
                    .as_str()
                    .unwrap_or(&record_kind)
                    .to_owned(),
                text: decode_envelope(arguments),
                decoded: true,
            });
        }
    }
    fields
}

/// Outcome of reading one bounded line.
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    /// Complete line (newline stripped) held in the buffer.
    Complete,
    /// The line exceeded the cap; it was consumed and discarded.
    Oversize,
    /// End of input (no bytes read).
    End,
    /// The byte or time budget ran out, possibly in the middle of a line.
    Exhausted,
}

/// Budget a bounded line read must respect, even inside one huge line.
#[derive(Debug, Clone, Copy)]
pub struct ReadLimit {
    /// Stop once this many bytes have been consumed in total.
    pub max_bytes: u64,
    /// Stop at this instant.
    pub deadline: Option<Instant>,
}

impl ReadLimit {
    /// No budget (tests and tools that bound input elsewhere).
    pub const NONE: Self = Self {
        max_bytes: u64::MAX,
        deadline: None,
    };

    fn spent(&self, bytes: u64) -> bool {
        bytes >= self.max_bytes || self.deadline.is_some_and(|d| Instant::now() >= d)
    }
}

/// Reads one `\n`-terminated line into `buffer`, holding at most `cap` bytes.
///
/// An oversized line is consumed to its newline without being stored, so
/// memory stays bounded however large a record is. `bytes` counts every byte
/// consumed.
pub fn read_line_capped<R: BufRead>(
    reader: &mut R,
    buffer: &mut Vec<u8>,
    cap: usize,
    bytes: &mut u64,
    limit: &ReadLimit,
) -> std::io::Result<Line> {
    buffer.clear();
    let mut oversize = false;
    let mut any = false;
    loop {
        if limit.spent(*bytes) {
            buffer.clear();
            return Ok(Line::Exhausted);
        }
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        // Never consume past the byte budget, however large the buffered chunk is.
        let room = usize::try_from(limit.max_bytes - *bytes).unwrap_or(usize::MAX);
        let available = &available[..available.len().min(room)];
        any = true;
        let (take, done) = match available.iter().position(|b| *b == b'\n') {
            Some(position) => (position + 1, true),
            None => (available.len(), false),
        };
        let body = if done { take - 1 } else { take };
        if !oversize {
            if buffer.len() + body > cap {
                oversize = true;
                buffer.clear();
            } else {
                buffer.extend_from_slice(&available[..body]);
            }
        }
        reader.consume(take);
        *bytes += take as u64;
        if done {
            break;
        }
    }
    Ok(match (any, oversize) {
        (false, _) => Line::End,
        (true, true) => Line::Oversize,
        (true, false) => {
            if buffer.last() == Some(&b'\r') {
                buffer.pop();
            }
            Line::Complete
        }
    })
}
