//! `recall`: bounded, read-only search over one chat's saved originals.
//!
//! Compaction replaces old messages with a summary but keeps the originals in
//! versioned JSONL snapshots under the chat's evidence directory
//! (`.themis/context/<thread>/snapshot-*.jsonl`), plus exported
//! `history-source-*.jsonl` exchanges and the attachment originals the chat
//! referenced. This tool searches exactly those sources, nothing else:
//!
//! - confinement: only regular, non-symlink `snapshot-*.jsonl` and
//!   `history-source-*.jsonl` files inside the evidence directory, and the
//!   attachment files this chat referenced that resolve inside the project's
//!   `.themis/attachments` directory;
//! - bounded work: files are stream-read line by line (an oversized record is
//!   skipped, not loaded), long fields are scanned in overlapping chunks, and a
//!   byte and time budget stops the scan with `truncated_scan: true`;
//! - deterministic ranking: exact phrase first, then passages by the summed
//!   inverse document frequency of the terms they match (a rare term counts
//!   more than several common ones, so a whole question still finds the
//!   distinctive passage), then all terms before some terms; ties break by
//!   recency (newer snapshot, later record) and coordinates;
//! - identical excerpts are reported once, from the newest source containing
//!   them;
//! - excerpts are untrusted data. Results carry coordinates that stay valid
//!   for `read_file` (`jsonl_record`, `json_pointer`, `offset`, and
//!   `decode_tool_payload` when `decoded` is true).

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use autoagents_derive::{tool, ToolInput};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{async_trait, parse_tool_args, tool_error, ToolCallError, ToolRuntime, ToolT};
use matching::{best_windows, scan_text};
pub use matching::{fold_char, Query};
pub use records::{decode_envelope, read_line_capped, record_fields, Line, ReadLimit, RecordField};
use sources::{
    attachment_sources, display_path, evidence_sources, open_regular, sha256_hex, Source,
    SourceKind,
};

/// Hits returned when `limit` is omitted.
pub const DEFAULT_LIMIT: usize = 5;
/// Largest number of hits one call returns.
pub const MAX_LIMIT: usize = 10;
/// Characters in one excerpt.
pub const EXCERPT_CHARS: usize = 600;
/// Characters returned by one `around` expansion.
pub const AROUND_CHARS: usize = 3000;
/// Serialized byte budget of one result (the delivery limit is 16 KiB).
pub const RESULT_BYTES: usize = 12_000;
/// Bytes read from all sources in one call.
pub const MAX_SCAN_BYTES: u64 = 512 * 1024 * 1024;
/// Largest JSONL record that is parsed; larger ones are skipped.
pub const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;
/// Largest attachment original that is searched.
pub const MAX_ATTACHMENT_BYTES: u64 = 64 * 1024 * 1024;
/// Wall-clock budget of one call.
pub const SCAN_DEADLINE: Duration = Duration::from_secs(10);
/// Evidence files considered per call (newest first).
const MAX_FILES: usize = 2000;
/// Characters scanned per chunk of a long field.
const CHUNK_CHARS: usize = 1 << 20;
/// Characters shared by adjacent chunks, so a phrase never straddles a cut.
const CHUNK_OVERLAP: usize = 4096;
/// Query terms that are scored.
const MAX_TERMS: usize = 16;
/// Query tokens that form the phrase.
const MAX_PHRASE: usize = 24;
/// Match occurrences kept per field.
const MAX_OCCURRENCES: usize = 20_000;
/// Excerpt windows reported per field.
const WINDOWS_PER_FIELD: usize = 3;
/// Candidate hits kept before ranking.
const MAX_CANDIDATES: usize = 20_000;
/// Span (in characters) within which matches count as one passage.
const WINDOW_SPAN: usize = 400;

const UNTRUSTED_NOTE: &str = crate::prompts::TOOL_RECALL_UNTRUSTED;

/// Evidence kinds searched when `kinds` is omitted.
pub const DEFAULT_KINDS: [&str; 2] = ["original", "original_exchange"];
const KNOWN_KINDS: [&str; 4] = ["original", "original_exchange", "navigation", "recovery"];

// ---------------------------------------------------------------------------
// Tool
// ---------------------------------------------------------------------------

/// Arguments for [`RecallTool`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct RecallArgs {
    /// Keywords or a phrase.
    #[serde(default)]
    #[input(
        description = "2-5 distinctive keywords or an exact phrase from the passage you need, not the whole question (empty only with around)"
    )]
    pub query: String,
    /// Hits to return.
    #[serde(default)]
    #[input(description = "Hits to return (default 5, max 10)")]
    pub limit: u64,
    /// Evidence kinds to search.
    #[serde(default)]
    #[input(
        description = "Usually leave empty (searches original and original_exchange). Only to add navigation or recovery copies"
    )]
    pub kinds: Vec<String>,
    /// Source filter.
    #[serde(default)]
    #[input(
        description = "Usually leave empty. Narrows to sources whose file name, attachment, tool name or role (User, Assistant, Tool) contains this; not a kind"
    )]
    pub source: String,
    /// A previous hit's `around` value.
    #[serde(default)]
    #[input(description = "A hit's `around` value: read more text around it")]
    pub around: String,
}

/// What one chat's recall may read.
#[derive(Debug, Clone)]
pub struct RecallScope {
    /// The chat's evidence directory (`.themis/context/<thread>`).
    pub evidence_dir: PathBuf,
    /// Attachment originals this chat referenced.
    pub attachments: Vec<PathBuf>,
    /// Directory attachments must live in (`<project>/.themis/attachments`).
    pub attachment_root: Option<PathBuf>,
    /// Project root; reported paths are relative to it when possible.
    pub project_root: Option<PathBuf>,
}

/// Searches this chat's saved originals.
#[tool(
    name = "recall",
    description = "Call before answering about earlier messages, attachments or tool output not in context. Searches this chat's saved originals; returns ranked untrusted excerpts with coordinates for follow-up calls (never shown to the user). Read-only.",
    input = RecallArgs,
)]
pub struct RecallTool {
    scope: Arc<RecallScope>,
}

impl RecallTool {
    /// Tool bound to one chat's sources.
    pub fn new(scope: RecallScope) -> Self {
        Self {
            scope: Arc::new(scope),
        }
    }
}

#[async_trait]
impl ToolRuntime for RecallTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: RecallArgs = parse_tool_args("recall", args)?;
        let scope = Arc::clone(&self.scope);
        tokio::task::spawn_blocking(move || recall(&scope, &args, &Limits::default()))
            .await
            .map_err(|error| tool_error(format!("recall: {error}")))?
            .map_err(|error| tool_error(format!("recall: {error}")))
    }
}

/// Scan budgets; tests lower them.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Bytes read from all sources.
    pub scan_bytes: u64,
    /// Wall-clock budget.
    pub deadline: Duration,
    /// Largest parsed record.
    pub record_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            scan_bytes: MAX_SCAN_BYTES,
            deadline: SCAN_DEADLINE,
            record_bytes: MAX_RECORD_BYTES,
        }
    }
}

/// Runs one recall call: a search, or an `around` expansion.
pub fn recall(scope: &RecallScope, args: &RecallArgs, limits: &Limits) -> Result<Value, String> {
    let kinds: Vec<String> = if args.kinds.is_empty() {
        DEFAULT_KINDS.iter().map(|k| (*k).to_owned()).collect()
    } else {
        let mut kinds = Vec::new();
        for kind in &args.kinds {
            let kind = kind.trim().to_ascii_lowercase();
            if !KNOWN_KINDS.contains(&kind.as_str()) {
                return Err(format!(
                    "unknown kind '{kind}' (use original, original_exchange, navigation or recovery)"
                ));
            }
            kinds.push(kind);
        }
        kinds
    };
    // Models often send every optional field with a placeholder ("0", "none"); a
    // real token always carries `|` separators, so anything else is not an expansion.
    let token = args.around.trim();
    if token.contains('|') || (!token.is_empty() && args.query.trim().is_empty()) {
        return around(scope, token, &kinds, limits);
    }
    let limit = match args.limit {
        0 => DEFAULT_LIMIT,
        n => usize::try_from(n).unwrap_or(MAX_LIMIT).min(MAX_LIMIT),
    };
    let query = Query::new(&args.query);
    if query.terms.is_empty() {
        return Err("query needs at least one word".into());
    }
    let source = args.source.trim();
    let mut found = search(scope, &query, &args.query, limit, &kinds, source, limits);
    // Models often narrow with guessed filters (a kind name as `source`, one kind only)
    // and then conclude the detail was never said. When the filters match nothing but the
    // default search does, return those hits and say the filters were ignored.
    let filtered = !source.is_empty() || kinds.iter().map(String::as_str).ne(DEFAULT_KINDS);
    if filtered && found["total_matches"].as_u64() == Some(0) {
        let defaults: Vec<String> = DEFAULT_KINDS.iter().map(|k| (*k).to_owned()).collect();
        let broad = search(scope, &query, &args.query, limit, &defaults, "", limits);
        if broad["total_matches"].as_u64().unwrap_or(0) > 0 {
            found = broad;
            found["filters_ignored"] =
                serde_json::json!(crate::prompts::TOOL_RECALL_FILTERS_IGNORED);
        }
    }
    Ok(found)
}

mod matching;
mod records;
mod sources;

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Candidate {
    tier: u8,
    distinct: usize,
    mask: u32,
    recency: u128,
    record: u64,
    pointer: String,
    start: usize,
    value: Value,
}

struct Budget {
    started: Instant,
    limits: Limits,
    bytes: u64,
    truncated: Vec<&'static str>,
}

impl Budget {
    fn new(limits: &Limits) -> Self {
        Self {
            started: Instant::now(),
            limits: limits.clone(),
            bytes: 0,
            truncated: Vec::new(),
        }
    }

    fn exhausted(&mut self) -> bool {
        if self.bytes >= self.limits.scan_bytes {
            self.note("scan_byte_budget");
            return true;
        }
        if self.started.elapsed() >= self.limits.deadline {
            self.note("time_budget");
            return true;
        }
        false
    }

    fn read_limit(&self) -> ReadLimit {
        ReadLimit {
            max_bytes: self.limits.scan_bytes,
            deadline: Some(self.started + self.limits.deadline),
        }
    }

    fn note(&mut self, reason: &'static str) {
        if !self.truncated.contains(&reason) {
            self.truncated.push(reason);
        }
    }
}

fn extract(text: &str, start: usize, end: usize) -> String {
    text.chars().skip(start).take(end - start).collect()
}

struct Filters<'a> {
    kinds: &'a [String],
    source: String,
}

impl Filters<'_> {
    /// Attachments are filtered up front; saved records by field.
    fn accepts_source(&self, source: &Source, display: &str) -> bool {
        if source.kind != SourceKind::Attachment {
            return true;
        }
        self.kinds.iter().any(|k| k == "original")
            && (self.source.is_empty() || self.names(source, display, None))
    }

    fn names(&self, source: &Source, display: &str, field: Option<&RecordField>) -> bool {
        let needle = self.source.to_lowercase();
        let mut haystacks = vec![source.name.to_lowercase(), display.to_lowercase()];
        if let Some(field) = field {
            haystacks.push(field.role.to_lowercase());
            if let Some(tool) = &field.tool {
                haystacks.push(tool.to_lowercase());
            }
        }
        haystacks.iter().any(|h| h.contains(&needle))
    }

    fn accepts_field(&self, source: &Source, display: &str, field: &RecordField) -> bool {
        self.kinds.contains(&field.evidence_kind)
            && (self.source.is_empty() || self.names(source, display, Some(field)))
    }
}

/// Everything one source scan reads or updates.
struct Scan<'a> {
    query: &'a Query,
    filters: &'a Filters<'a>,
    budget: Budget,
    candidates: Vec<Candidate>,
    seen: HashSet<[u8; 32]>,
    records: u64,
    /// Searched fields, and how many of them contain each query term.
    fields: u64,
    document_frequency: Vec<u64>,
}

/// Inverse document frequency of each query term over the searched fields
/// (BM25 form, always positive), in thousandths so ranking keys stay integral.
fn term_weights(fields: u64, document_frequency: &[u64]) -> Vec<u64> {
    document_frequency
        .iter()
        .map(|&found| {
            let rest = fields.saturating_sub(found) as f64;
            // A matched term always counts, even when every field contains it.
            (((1.0 + (rest + 0.5) / (found as f64 + 0.5)).ln() * 1000.0).round() as u64).max(1)
        })
        .collect()
}

fn search(
    scope: &RecallScope,
    query: &Query,
    raw_query: &str,
    limit: usize,
    kinds: &[String],
    source_filter: &str,
    limits: &Limits,
) -> Value {
    let filters = Filters {
        kinds,
        source: source_filter.to_owned(),
    };
    let mut scan = Scan {
        query,
        filters: &filters,
        budget: Budget::new(limits),
        candidates: Vec::new(),
        seen: HashSet::new(),
        records: 0,
        fields: 0,
        document_frequency: vec![0; query.terms.len()],
    };
    let (mut sources, too_many_files) = evidence_sources(scope);
    if too_many_files {
        scan.budget.note("file_count");
    }
    sources.extend(attachment_sources(scope));
    let mut counts = [0usize; 3];
    for source in &sources {
        if scan.budget.exhausted() {
            break;
        }
        let display = display_path(scope, &source.path);
        if !filters.accepts_source(source, &display) {
            continue;
        }
        counts[source.kind as usize] += 1;
        if source.kind == SourceKind::Attachment {
            scan_attachment(source, &display, &mut scan);
        } else {
            scan_jsonl(source, &display, &mut scan);
        }
        if scan.candidates.len() >= MAX_CANDIDATES {
            scan.budget.note("candidate_cap");
            break;
        }
    }
    let total = scan.candidates.len();
    let weights = term_weights(scan.fields, &scan.document_frequency);
    let weight = |mask: u32| -> u64 {
        weights
            .iter()
            .enumerate()
            .filter(|(term, _)| mask & (1 << term) != 0)
            .map(|(_, weight)| weight)
            .sum()
    };
    scan.candidates.sort_by(|a, b| {
        let key = |c: &Candidate| {
            (
                c.tier == 3,
                weight(c.mask),
                c.tier,
                c.distinct,
                c.recency,
                c.record,
            )
        };
        key(b)
            .cmp(&key(a))
            .then_with(|| a.pointer.cmp(&b.pointer))
            .then_with(|| a.start.cmp(&b.start))
    });
    let mut hits: Vec<Value> = scan
        .candidates
        .into_iter()
        .take(limit)
        .enumerate()
        .map(|(rank, mut candidate)| {
            candidate.value["rank"] = (rank + 1).into();
            candidate.value
        })
        .collect();
    let (budget, records) = (scan.budget, scan.records);
    let build = |hits: &[Value], omitted: usize| {
        json!({
            "query": raw_query.chars().take(200).collect::<String>(),
            "untrusted": true,
            "note": UNTRUSTED_NOTE,
            "hits": hits,
            "total_matches": total,
            "omitted_for_size": omitted,
            "scanned": {"snapshots": counts[0], "history_sources": counts[1], "attachments": counts[2], "records": records, "bytes": budget.bytes},
            "truncated_scan": !budget.truncated.is_empty(),
            "truncated_reasons": budget.truncated,
        })
    };
    let mut omitted = 0;
    let mut result = build(&hits, omitted);
    while result.to_string().len() > RESULT_BYTES && hits.len() > 1 {
        hits.pop();
        omitted += 1;
        result = build(&hits, omitted);
    }
    result
}

/// Scores one field and records its best passages.
fn consider_field(
    source: &Source,
    display: &str,
    record: u64,
    record_sha: &mut Option<String>,
    line: &[u8],
    field: &RecordField,
    scan: &mut Scan,
) {
    scan.fields += 1;
    let found = scan_text(
        &field.text,
        scan.query,
        Some(scan.budget.started + scan.budget.limits.deadline),
    );
    if found.capped {
        scan.budget.note("occurrence_cap");
    }
    if found.timed_out {
        scan.budget.note("time_budget");
    }
    if found.occurrences.is_empty() {
        return;
    }
    let present = found
        .occurrences
        .iter()
        .fold(0u32, |mask, occurrence| mask | 1 << occurrence.term);
    for (term, count) in scan.document_frequency.iter_mut().enumerate() {
        *count += u64::from(present & (1 << term) != 0);
    }
    let total_chars = field.text.chars().count();
    for window in best_windows(&found, scan.query.terms.len()) {
        let span = window.end - window.start;
        let room = EXCERPT_CHARS.saturating_sub(span.min(EXCERPT_CHARS)) / 2;
        let end = (window.start.saturating_sub(room) + EXCERPT_CHARS).min(total_chars);
        let start = end.saturating_sub(EXCERPT_CHARS).min(window.start);
        let end = (start + EXCERPT_CHARS).min(total_chars);
        let excerpt = extract(&field.text, start, end);
        // Same text from a different role or evidence kind is a different observation.
        let digest: [u8; 32] = Sha256::new()
            .chain_update(field.role.as_bytes())
            .chain_update([0])
            .chain_update(field.evidence_kind.as_bytes())
            .chain_update([0])
            .chain_update(excerpt.as_bytes())
            .finalize()
            .into();
        if !scan.seen.insert(digest) {
            continue;
        }
        let sha = record_sha.get_or_insert_with(|| sha256_hex(line)).clone();
        let match_kind = match window.tier {
            3 => "phrase",
            2 => "all_terms",
            _ => "some_terms",
        };
        let is_attachment = source.kind == SourceKind::Attachment;
        let center = window.start + span / 2;
        scan.candidates.push(Candidate {
            tier: window.tier,
            distinct: window.distinct,
            mask: window.mask,
            recency: source.recency,
            record,
            pointer: field.pointer.clone(),
            start: window.start,
            value: json!({
                "match": match_kind,
                "terms_matched": window.distinct,
                "excerpt": excerpt,
                "source": source.name,
                "kind": source.kind.label(),
                "snapshot": (source.kind == SourceKind::Snapshot).then(|| source.name.clone()),
                "path": display,
                "record": (!is_attachment).then_some(record),
                "pointer": field.pointer,
                "char_range": [start, end],
                "match_range": [window.start, window.end],
                "decoded": field.decoded,
                "role": field.role,
                "tool": field.tool,
                "evidence_kind": field.evidence_kind,
                "sha256": sha,
                "around": format!("{display}|{record}|{}|{center}", field.pointer),
            }),
        });
    }
}

fn scan_jsonl(source: &Source, display: &str, scan: &mut Scan) {
    let Some(file) = open_regular(&source.path) else {
        return;
    };
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut line = Vec::new();
    let mut record = 0u64;
    loop {
        if scan.budget.exhausted() || scan.candidates.len() >= MAX_CANDIDATES {
            break;
        }
        let cap = scan.budget.limits.record_bytes;
        let limit = scan.budget.read_limit();
        match read_line_capped(&mut reader, &mut line, cap, &mut scan.budget.bytes, &limit) {
            Ok(Line::End) | Err(_) => break,
            Ok(Line::Exhausted) => {
                scan.budget.exhausted();
                break;
            }
            Ok(Line::Oversize) => {
                record += 1;
                scan.records += 1;
                scan.budget.note("oversized_record");
            }
            Ok(Line::Complete) => {
                record += 1;
                scan.records += 1;
                let mut record_sha = None;
                for field in record_fields(&line) {
                    if scan.filters.accepts_field(source, display, &field) {
                        consider_field(
                            source,
                            display,
                            record,
                            &mut record_sha,
                            &line,
                            &field,
                            scan,
                        );
                    }
                }
            }
        }
    }
}

/// Reads an attachment original as UTF-8 text with its SHA-256; `None` for
/// binary, oversized or over-budget files.
fn read_attachment(path: &Path, budget: &mut Budget) -> Option<(String, String)> {
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > MAX_ATTACHMENT_BYTES {
        budget.note("attachment_too_large");
        return None;
    }
    if budget.bytes.saturating_add(metadata.len()) > budget.limits.scan_bytes {
        budget.note("scan_byte_budget");
        return None;
    }
    let mut bytes = Vec::new();
    open_regular(path)?
        .take(MAX_ATTACHMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    budget.bytes += bytes.len() as u64;
    let sha = sha256_hex(&bytes);
    let text = String::from_utf8(bytes)
        .ok()
        .filter(|text| !text.contains('\0'))?;
    Some((text, sha))
}

fn scan_attachment(source: &Source, display: &str, scan: &mut Scan) {
    let Some((text, sha)) = read_attachment(&source.path, &mut scan.budget) else {
        return;
    };
    scan.records += 1;
    let field = RecordField {
        pointer: String::new(),
        role: "User".into(),
        tool: None,
        evidence_kind: "original".into(),
        text,
        decoded: false,
    };
    let mut record_sha = Some(sha);
    consider_field(source, display, 0, &mut record_sha, b"", &field, scan);
}

// ---------------------------------------------------------------------------
// Around
// ---------------------------------------------------------------------------

fn around(
    scope: &RecallScope,
    token: &str,
    kinds: &[String],
    limits: &Limits,
) -> Result<Value, String> {
    let invalid = || "around must be a hit's `around` value".to_owned();
    let mut parts = token.rsplitn(4, '|');
    let center: usize = parts
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or_else(invalid)?;
    let pointer = parts.next().ok_or_else(invalid)?.to_owned();
    let record: u64 = parts
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or_else(invalid)?;
    let path = parts.next().ok_or_else(invalid)?.to_owned();
    let mut budget = Budget::new(limits);
    let (mut sources, _) = evidence_sources(scope);
    sources.extend(attachment_sources(scope));
    let source = sources
        .iter()
        .find(|s| display_path(scope, &s.path) == path)
        .ok_or("around names no source of this chat")?
        .clone();
    let is_attachment = source.kind == SourceKind::Attachment;
    let (text, sha, role, evidence_kind, tool, decoded) = if is_attachment {
        let (text, sha) = read_attachment(&source.path, &mut budget)
            .ok_or("attachment is not readable text within the limits")?;
        (
            text,
            sha,
            "User".to_owned(),
            "original".to_owned(),
            None,
            false,
        )
    } else {
        let file = open_regular(&source.path).ok_or("source is not a regular file")?;
        let mut reader = BufReader::with_capacity(64 * 1024, file);
        let mut line = Vec::new();
        let mut number = 0u64;
        while number < record {
            if budget.exhausted() {
                return Err("scan budget exhausted before the record".into());
            }
            let cap = limits.record_bytes;
            let read_limit = budget.read_limit();
            match read_line_capped(&mut reader, &mut line, cap, &mut budget.bytes, &read_limit)
                .map_err(|e| e.to_string())?
            {
                Line::End => return Err("record not found".into()),
                Line::Exhausted => return Err("scan budget exhausted before the record".into()),
                Line::Oversize | Line::Complete => number += 1,
            }
        }
        if record == 0 {
            return Err("record numbers start at 1".into());
        }
        let field = record_fields(&line)
            .into_iter()
            .find(|field| field.pointer == pointer)
            .ok_or("pointer not found in record")?;
        (
            field.text,
            sha256_hex(&line),
            field.role,
            field.evidence_kind,
            field.tool,
            field.decoded,
        )
    };
    if !kinds.contains(&evidence_kind) {
        return Err(format!(
            "the record is evidence_kind {evidence_kind}; pass kinds to include it"
        ));
    }
    let total = text.chars().count();
    let start = center.saturating_sub(AROUND_CHARS / 3).min(total);
    let mut end = (start + AROUND_CHARS).min(total);
    let link = |offset: usize| format!("{path}|{record}|{pointer}|{offset}");
    loop {
        let result = json!({
            "untrusted": true,
            "note": UNTRUSTED_NOTE,
            "excerpt": extract(&text, start, end),
            "source": source.name,
            "kind": source.kind.label(),
            "path": path,
            "record": (!is_attachment).then_some(record),
            "pointer": pointer,
            "char_range": [start, end],
            "total_chars": total,
            "decoded": decoded,
            "role": role,
            "tool": tool,
            "evidence_kind": evidence_kind,
            "sha256": sha,
            // Neighbouring pages start exactly where this one ends or begins.
            "before": (start > 0).then(|| link(start.saturating_sub(AROUND_CHARS * 2 / 3))),
            "after": (end < total).then(|| link(end + AROUND_CHARS / 3)),
        });
        // Escaping can inflate an excerpt; stay under the result delivery budget.
        if result.to_string().len() <= RESULT_BYTES || end <= start + 1 {
            return Ok(result);
        }
        end = start + (end - start) * 3 / 4;
    }
}

#[cfg(test)]
mod tests;
