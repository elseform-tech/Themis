//! Channel logs: `channels/<channel id>.jsonl`, one `{seq, at_ms, entry}` per line, seqs
//! rising from 1.
//!
//! A page is read from the end of the file in chunks, never the whole log. To start before a
//! given seq, a binary search over byte offsets finds that seq's line, reading one or two lines
//! per probe, so a page costs about the same at any depth of any log. A torn last line (a crash
//! mid-append) is skipped when reading, and the next append starts on a fresh line.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::store::{channel_log_path, refuse_symlink, StoreError};
use super::{ChannelEntry, VisionsCache, CHANNELS_FILE};

/// How diagnostics and errors name a log.
const LOG_FILE: &str = "channel log";
/// Chunk size for reading back from the end.
const BACK_CHUNK: u64 = 8 * 1024;
/// Chunk size for reading one line forward while searching.
const LINE_CHUNK: u64 = 512;
/// The longest line an append writes; longer lines are damage and are skipped.
const MAX_LINE_BYTES: usize = 64 * 1024;

/// One line of a channel log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelLine {
    pub seq: u64,
    pub at_ms: u64,
    pub entry: ChannelEntry,
}

/// Entries oldest first, and the `before_seq` that continues back (None at the start).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ChannelPage {
    pub entries: Vec<ChannelLine>,
    pub next_before: Option<u64>,
}

#[cfg(test)]
thread_local! {
    static BYTES_READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Bytes read from logs on this thread so far, which starts the count again.
#[cfg(test)]
pub(super) fn take_bytes_read() -> u64 {
    BYTES_READ.take()
}

fn unreadable(error: &std::io::Error) -> StoreError {
    StoreError::unreadable(LOG_FILE, &error.kind().to_string())
}

/// Reads bytes `start..end`.
fn read_range(file: &mut File, start: u64, end: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = vec![0; (end - start) as usize];
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(&mut bytes)?;
    #[cfg(test)]
    BYTES_READ.with(|read| read.set(read.get() + bytes.len() as u64));
    Ok(bytes)
}

/// The line starting at `start` (without its newline) and where the next line starts. Past
/// [`MAX_LINE_BYTES`] it gives up on the line and goes on from there.
fn read_line(file: &mut File, start: u64, len: u64) -> std::io::Result<(Vec<u8>, u64)> {
    let mut line = Vec::new();
    let mut at = start;
    while at < len && line.len() <= MAX_LINE_BYTES {
        let chunk = read_range(file, at, len.min(at + LINE_CHUNK))?;
        if let Some(newline) = chunk.iter().position(|&byte| byte == b'\n') {
            line.extend_from_slice(&chunk[..newline]);
            return Ok((line, at + newline as u64 + 1));
        }
        at += chunk.len() as u64;
        line.extend_from_slice(&chunk);
    }
    Ok((line, at))
}

/// The first readable line starting at or after `pos`: its start and seq.
fn probe(file: &mut File, pos: u64, len: u64) -> std::io::Result<Option<(u64, u64)>> {
    let mut start = if pos == 0 {
        0
    } else {
        read_line(file, pos - 1, len)?.1
    };
    while start < len {
        let (bytes, next) = read_line(file, start, len)?;
        if let Ok(line) = serde_json::from_slice::<ChannelLine>(&bytes) {
            return Ok(Some((start, line.seq)));
        }
        start = next;
    }
    Ok(None)
}

/// Where the first line with a seq of `before` or more starts (the end when there is none).
fn boundary(file: &mut File, len: u64, before: u64) -> std::io::Result<u64> {
    let (mut low, mut high) = (0, len);
    while low < high {
        let middle = low + (high - low) / 2;
        match probe(file, middle, len)? {
            Some((_, seq)) if seq < before => low = middle + 1,
            _ => high = middle,
        }
    }
    Ok(probe(file, low, len)?.map_or(len, |(start, _)| start))
}

/// Gives `visit` the lines that end before `end`, newest first, until it returns false.
/// Returns whether it reached the start of the file.
fn lines_back(
    file: &mut File,
    end: u64,
    mut visit: impl FnMut(&[u8]) -> bool,
) -> std::io::Result<bool> {
    let mut pos = end;
    let mut carry: Vec<u8> = Vec::new();
    // Set while skipping the rest of a line already over the limit.
    let mut oversized = false;
    loop {
        if pos == 0 {
            return Ok(oversized || carry.is_empty() || visit(&carry));
        }
        let from = pos.saturating_sub(BACK_CHUNK);
        let mut bytes = read_range(file, from, pos)?;
        pos = from;
        bytes.append(&mut carry);
        let mut cut = bytes.len();
        for index in (0..bytes.len()).rev() {
            if bytes[index] != b'\n' {
                continue;
            }
            let line = &bytes[index + 1..cut];
            if !std::mem::take(&mut oversized) && !line.is_empty() && !visit(line) {
                return Ok(false);
            }
            cut = index;
        }
        bytes.truncate(cut);
        if bytes.len() > MAX_LINE_BYTES {
            oversized = true;
        } else if !oversized {
            carry = bytes;
        }
    }
}

/// Fails when the `channels/` folder holding `path`, or the `visions/` folder above it, is a
/// symlink.
fn refuse_linked_folders(path: &Path) -> Result<(), StoreError> {
    let channels = path.parent();
    for folder in [channels, channels.and_then(Path::parent)]
        .into_iter()
        .flatten()
    {
        refuse_symlink(folder, LOG_FILE)?;
    }
    Ok(())
}

/// The log at `path` and its length, or None when there is no log yet.
fn open_log(path: &Path) -> Result<Option<(File, u64)>, StoreError> {
    refuse_linked_folders(path)?;
    refuse_symlink(path, LOG_FILE)?;
    match File::open(path) {
        Ok(file) => {
            let len = file.metadata().map_err(|error| unreadable(&error))?.len();
            Ok(Some((file, len)))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(unreadable(&error)),
    }
}

/// Collects a page's entries, newest first, while they fit.
struct PageFill {
    before_seq: Option<u64>,
    max_bytes: usize,
    used: usize,
    entries: Vec<ChannelLine>,
}

impl PageFill {
    /// Takes one line; false once the page is full. Unreadable lines are skipped.
    fn take(&mut self, bytes: &[u8]) -> bool {
        let Ok(line) = serde_json::from_slice::<ChannelLine>(bytes) else {
            return true;
        };
        if self.before_seq.is_some_and(|before| line.seq >= before) {
            return true;
        }
        let size = bytes.len() + 1;
        if !self.entries.is_empty() && self.used + size > self.max_bytes {
            return false;
        }
        self.used += size;
        self.entries.push(line);
        true
    }
}

/// A page of the log at `path`; see [`super::AppState::channel_page`].
pub(super) fn read_page(
    path: &Path,
    before_seq: Option<u64>,
    max_bytes: usize,
) -> Result<ChannelPage, StoreError> {
    let Some((mut file, len)) = open_log(path)? else {
        return Ok(ChannelPage::default());
    };
    let end = match before_seq {
        Some(before) => boundary(&mut file, len, before).map_err(|error| unreadable(&error))?,
        None => len,
    };
    let mut fill = PageFill {
        before_seq,
        max_bytes,
        used: 0,
        entries: Vec::new(),
    };
    let reached_start =
        lines_back(&mut file, end, |bytes| fill.take(bytes)).map_err(|error| unreadable(&error))?;
    let mut entries = fill.entries;
    entries.reverse();
    let next_before = if reached_start {
        None
    } else {
        entries.first().map(|line| line.seq)
    };
    Ok(ChannelPage {
        entries,
        next_before,
    })
}

/// The newest readable seq in the log (0 for none), and whether the file ends mid-line.
fn tail(path: &Path) -> Result<(u64, bool), StoreError> {
    let newest = read_page(path, None, 0)?;
    let seq = newest.entries.last().map_or(0, |line| line.seq);
    let Some((mut file, len)) = open_log(path)? else {
        return Ok((seq, false));
    };
    let torn = len > 0
        && read_range(&mut file, len - 1, len).map_err(|error| unreadable(&error))? != b"\n";
    Ok((seq, torn))
}

fn append_bytes(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let write_failed = |error: std::io::Error| StoreError::write_failed(LOG_FILE, &error);
    let dir = path.parent().unwrap_or(path);
    refuse_linked_folders(path)?;
    std::fs::create_dir_all(dir).map_err(write_failed)?;
    refuse_symlink(path, LOG_FILE)?;
    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(write_failed)?;
    file.write_all(bytes).map_err(write_failed)?;
    file.sync_data().map_err(write_failed)
}

fn now_ms() -> u64 {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
}

impl VisionsCache {
    /// Appends `entry` to a saved channel's log and returns its seq.
    pub(super) fn append_entry(
        &mut self,
        dir: &Path,
        channel_id: &str,
        entry: ChannelEntry,
    ) -> Result<u64, StoreError> {
        let path = channel_log_path(dir, channel_id)?;
        if !self.channels(dir)?.contains_key(channel_id) {
            return Err(StoreError::refused(CHANNELS_FILE, "Unknown channel"));
        }
        let (last, torn) = match self.last_seq.get(channel_id) {
            Some(&seq) => (seq, false),
            None => tail(&path)?,
        };
        let line = ChannelLine {
            seq: last + 1,
            at_ms: now_ms(),
            entry,
        };
        let mut bytes = Vec::new();
        if torn {
            bytes.push(b'\n');
        }
        serde_json::to_writer(&mut bytes, &line)
            .map_err(|_| StoreError::refused(LOG_FILE, "The entry could not be encoded"))?;
        if bytes.len() > MAX_LINE_BYTES {
            return Err(StoreError::refused(
                LOG_FILE,
                "The channel entry is too long",
            ));
        }
        bytes.push(b'\n');
        if let Err(error) = append_bytes(&path, &bytes) {
            // Some of the line may have landed: the next append reads the tail again.
            self.last_seq.remove(channel_id);
            return Err(error);
        }
        self.last_seq.insert(channel_id.to_owned(), line.seq);
        Ok(line.seq)
    }
}
