//! The bounded rolling log a task's output goes to, read in pages that never split a character.

use super::*;

/// Last [`MAX_LOG_BYTES`] of a byte stream with absolute offsets, so readers
/// can page forward and learn how much rotated away.
#[derive(Debug, Clone)]
pub struct LogRing {
    bytes: VecDeque<u8>,
    capacity: usize,
    total: u64,
}

/// One page of a [`LogRing`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogChunk {
    /// Valid UTF-8 text for `offset..next_offset`.
    pub text: String,
    /// Absolute byte offset of the first byte of `text`.
    pub offset: u64,
    /// Offset to pass next to continue; equals `total` when caught up.
    pub next_offset: u64,
    /// Bytes written over the task's lifetime.
    pub total: u64,
    /// Bytes before `retained_from` rotated out of the ring.
    pub retained_from: u64,
}

/// Length of an incomplete UTF-8 sequence at the end of `bytes` (0 when none).
fn incomplete_tail(bytes: &[u8]) -> usize {
    for back in 1..=bytes.len().min(3) {
        let byte = bytes[bytes.len() - back];
        if byte & 0xC0 == 0x80 {
            continue;
        }
        let needed = match byte {
            0xF0..=0xF7 => 4,
            0xE0..=0xEF => 3,
            0xC0..=0xDF => 2,
            _ => 1,
        };
        return if needed > back { back } else { 0 };
    }
    0
}

impl LogRing {
    /// Ring keeping the last `capacity` bytes.
    pub fn new(capacity: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            capacity: capacity.max(1),
            total: 0,
        }
    }

    /// Ring pre-filled with `data` as its whole history.
    pub fn from_bytes(capacity: usize, data: &[u8]) -> Self {
        let mut ring = Self::new(capacity);
        ring.push(data);
        ring
    }

    /// Appends output, dropping the oldest bytes beyond capacity.
    pub fn push(&mut self, data: &[u8]) {
        self.total += data.len() as u64;
        if data.len() >= self.capacity {
            self.bytes.clear();
            self.bytes.extend(&data[data.len() - self.capacity..]);
            return;
        }
        self.bytes.extend(data);
        let excess = self.bytes.len().saturating_sub(self.capacity);
        self.bytes.drain(..excess);
    }

    /// Bytes written over the lifetime of the stream.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Absolute offset of the oldest retained byte.
    pub fn retained_from(&self) -> u64 {
        self.total - self.bytes.len() as u64
    }

    /// Bytes currently retained.
    pub fn retained_len(&self) -> usize {
        self.bytes.len()
    }

    /// Retained bytes as one vector (for persistence).
    pub fn to_vec(&self) -> Vec<u8> {
        self.bytes.iter().copied().collect()
    }

    /// Reads up to `max_bytes` from absolute offset `from` (clamped to what is
    /// retained). Never splits a character: leading continuation bytes are
    /// skipped and an incomplete trailing sequence waits for the next page.
    pub fn read(&self, from: u64, max_bytes: usize) -> LogChunk {
        self.read_with(from, max_bytes, false)
    }

    /// [`read`](Self::read); with `settled` the stream is over, so an
    /// incomplete character at the very end is final and decoded lossily
    /// instead of waiting for bytes that will never arrive.
    pub fn read_with(&self, from: u64, max_bytes: usize, settled: bool) -> LogChunk {
        let retained_from = self.retained_from();
        let mut start = from.clamp(retained_from, self.total);
        let skip = (start - retained_from) as usize;
        // Read a little past the budget so a page always holds one whole
        // character, even when `max_bytes` is smaller than that character.
        let mut slice: Vec<u8> = self
            .bytes
            .iter()
            .skip(skip)
            .take(max_bytes.max(4))
            .copied()
            .collect();
        let leading = slice
            .iter()
            .take_while(|byte| **byte & 0xC0 == 0x80)
            .count();
        // A page that begins inside a character resumes after it.
        slice.drain(..leading);
        start += leading as u64;
        let reaches_end = start + slice.len() as u64 >= self.total;
        let trim = if settled && reaches_end {
            0
        } else {
            incomplete_tail(&slice)
        };
        slice.truncate(slice.len() - trim);
        if slice.len() > max_bytes {
            // Cut back to the last whole character within the budget, but
            // never return an empty page while a character is available.
            let mut end = max_bytes;
            while end > 0 && slice[end] & 0xC0 == 0x80 {
                end -= 1;
            }
            if end == 0 {
                end = 1;
                while end < slice.len() && slice[end] & 0xC0 == 0x80 {
                    end += 1;
                }
            }
            slice.truncate(end);
        }
        let next_offset = start + slice.len() as u64;
        LogChunk {
            text: String::from_utf8_lossy(&slice).into_owned(),
            offset: start,
            next_offset,
            total: self.total,
            retained_from,
        }
    }

    /// The last `max_bytes` of output, aligned to a character boundary.
    pub fn tail(&self, max_bytes: usize) -> LogChunk {
        let from = self.total.saturating_sub(max_bytes as u64);
        self.read(from, max_bytes)
    }

    /// [`tail`](Self::tail) for a finished stream.
    pub fn tail_settled(&self, max_bytes: usize) -> LogChunk {
        let from = self.total.saturating_sub(max_bytes as u64);
        self.read_with(from, max_bytes, true)
    }

    /// Up to `lines` most recent non-empty lines, each bounded in length.
    pub fn tail_lines(&self, lines: usize) -> Vec<String> {
        self.tail_lines_complete(lines).0
    }

    /// [`tail_lines`](Self::tail_lines) plus whether they are the entire
    /// output (nothing rotated out, no line cut, nothing left out).
    pub fn tail_lines_complete(&self, lines: usize) -> (Vec<String>, bool) {
        let chunk = self.tail(8 * 1024);
        let mut complete = self.retained_from() == 0 && chunk.offset == 0;
        let mut found: Vec<String> = chunk
            .text
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .map(|line| {
                complete &= line.chars().count() <= MAX_TAIL_LINE_CHARS;
                line.chars().take(MAX_TAIL_LINE_CHARS).collect()
            })
            .collect();
        let excess = found.len().saturating_sub(lines);
        complete &= excess == 0 && chunk.next_offset >= chunk.total;
        found.drain(..excess);
        (found, complete)
    }
}
