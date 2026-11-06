use super::*;

/// Most of a transcript kept, in bytes: past this its older half goes.
const TRANSCRIPT_BYTES: u64 = 8 * 1024 * 1024;
/// Most of one entry kept in a transcript, in bytes.
pub(super) const TRANSCRIPT_ENTRY_BYTES: usize = 64 * 1024;

/// Appends one line, readable only by the person, keeping the newer half when it grows too long.
pub(super) fn append_transcript(path: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() > TRANSCRIPT_BYTES) {
        let kept = std::fs::read(path)?;
        let half = kept.len() / 2;
        let start = kept[half..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(kept.len(), |at| half + at + 1);
        std::fs::write(path, &kept[start..])?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")
}

/// `text` as one entry in a transcript, cut to its bound.
pub(super) fn entry_text(text: &str) -> &str {
    capped(text, TRANSCRIPT_ENTRY_BYTES)
}
