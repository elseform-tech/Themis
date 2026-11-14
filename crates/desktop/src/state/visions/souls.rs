//! Souls: who a chief or Atlas is and how it works, one `souls/<vision id>.md` per agent under
//! the Visions store. Workers have none (they have a role). The file is the person's: the
//! profile's Soul box edits it, `open_soul` opens it in their editor, and an edit made there is
//! what the next wake reads. A run reads it once, when its head is built, so a change applies
//! from the next wake and never to a run already going.
//!
//! A soul is at most [`SOUL_MAX_BYTES`] of UTF-8. A file that cannot be read (not text, too
//! big, a symlink) is reported as a problem on the profile and the run goes without it; it is
//! never silently replaced. Saves name the revision they were opened at (a hash of the text).

use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::json;

use super::{is_admin, AppState};

/// Under the Visions store.
const SOULS_DIR: &str = "souls";
/// The largest soul, in bytes.
pub(crate) const SOUL_MAX_BYTES: usize = 16 * 1024;

/// A soul as the profile shows it: its text, the revision to save against, and why it could
/// not be read, if so.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SoulView {
    pub text: String,
    pub revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// `visions/souls/<id>.md`, for an id the store could have given.
pub(super) fn soul_path(dir: &Path, vision_id: &str) -> Result<PathBuf, String> {
    if !crate::state::chat_grants::safe_chat_id(vision_id) {
        return Err("Unknown Vision".to_owned());
    }
    Ok(dir.join(SOULS_DIR).join(format!("{vision_id}.md")))
}

/// The soul's text (`None` when there is no file), or why it cannot be read.
pub(super) fn read_soul(dir: &Path, vision_id: &str) -> Result<Option<String>, String> {
    let path = soul_path(dir, vision_id)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("soul.md could not be read ({})", error.kind())),
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err("soul.md is a link; replace it with a file".to_owned())
        }
        Ok(_) => {}
    }
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .and_then(|file| file.take(SOUL_MAX_BYTES as u64 + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("soul.md could not be read ({})", error.kind()))?;
    if bytes.len() > SOUL_MAX_BYTES {
        return Err("soul.md is over 16 KiB; shorten it".to_owned());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "soul.md is not plain text".to_owned())
}

/// The revision of a soul's text: empty for no soul, else a hash of the text.
pub(super) fn revision_of(text: Option<&str>) -> String {
    match text {
        None => String::new(),
        Some(text) => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        }
    }
}

/// Writes the soul when the file is still at `expected` (fresh 0600 file, then rename).
pub(super) fn write_soul(
    dir: &Path,
    vision_id: &str,
    text: &str,
    expected: &str,
) -> Result<String, String> {
    if text.len() > SOUL_MAX_BYTES {
        return Err("A soul is at most 16 KiB".to_owned());
    }
    let current = read_soul(dir, vision_id)?;
    if revision_of(current.as_deref()) != expected {
        return Err("The soul changed since it was opened; reload before saving".to_owned());
    }
    let path = soul_path(dir, vision_id)?;
    let parent = path.parent().expect("souls dir");
    fs::create_dir_all(parent)
        .map_err(|error| format!("soul.md could not be saved ({})", error.kind()))?;
    let temp = parent.join(format!("{vision_id}.md.tmp"));
    let written = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut out = options.open(&temp)?;
        std::io::Write::write_all(&mut out, text.as_bytes())?;
        out.sync_all()?;
        fs::rename(&temp, &path)
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(format!("soul.md could not be saved ({})", error.kind()));
    }
    Ok(revision_of(Some(text)))
}

impl AppState {
    /// Whether `vision_id` may have a soul: Atlas, or the chief of a channel.
    fn has_soul(&self, vision_id: &str) -> bool {
        is_admin(vision_id)
            || self
                .channels_all()
                .iter()
                .any(|channel| channel.is_chief(vision_id))
    }

    /// The soul of a Vision as the profile shows it.
    pub fn get_soul(&self, vision_id: &str) -> Result<SoulView, String> {
        if self.vision_get(vision_id).is_none() {
            return Err("Unknown Vision".to_owned());
        }
        Ok(match read_soul(&self.inner.visions_dir, vision_id) {
            Ok(text) => SoulView {
                revision: revision_of(text.as_deref()),
                text: text.unwrap_or_default(),
                problem: None,
            },
            Err(problem) => SoulView {
                text: String::new(),
                revision: String::new(),
                problem: Some(problem),
            },
        })
    }

    /// Saves a chief's or Atlas's soul; a worker has none.
    pub fn save_soul(
        &self,
        vision_id: &str,
        text: &str,
        revision: &str,
    ) -> Result<SoulView, String> {
        if self.vision_get(vision_id).is_none() {
            return Err("Unknown Vision".to_owned());
        }
        if !self.has_soul(vision_id) {
            return Err("Only a channel's chief of staff and Atlas have a soul".to_owned());
        }
        let revision = write_soul(&self.inner.visions_dir, vision_id, text, revision)?;
        self.visions_note(
            "soul_saved",
            json!({"vision_id": vision_id, "bytes": text.len()}),
        );
        Ok(SoulView {
            text: text.to_owned(),
            revision,
            problem: None,
        })
    }

    /// Opens a chief's or Atlas's `soul.md` in the person's editor (`editor`, else the one
    /// Settings chose), making an empty one first when there is none.
    pub async fn open_soul(&self, vision_id: &str, editor: Option<String>) -> Result<(), String> {
        if self.vision_get(vision_id).is_none() || !self.has_soul(vision_id) {
            return Err("Only a channel's chief of staff and Atlas have a soul".to_owned());
        }
        let dir = &self.inner.visions_dir;
        if read_soul(dir, vision_id)?.is_none() {
            write_soul(dir, vision_id, "", "")?;
        }
        let editor = match editor {
            Some(editor) => editor,
            None => self.get_settings().await.config_editor,
        };
        crate::state::editors::open_with(&editor, &soul_path(dir, vision_id)?)
    }

    /// The soul a run's head carries, read now; empty when there is none or it cannot be read
    /// (the problem is logged, never its text).
    pub(super) fn soul_for_run(&self, vision_id: &str) -> String {
        match read_soul(&self.inner.visions_dir, vision_id) {
            Ok(text) => text.unwrap_or_default(),
            Err(_) => {
                self.visions_note("soul_unreadable", json!({"vision_id": vision_id}));
                String::new()
            }
        }
    }
}
