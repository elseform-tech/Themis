//! `attach_file`: the agent shows images, videos and PDFs inline in its reply.
//! Files come from the project (screenshots and recordings are saved there
//! already); anything outside the attachments folder is copied into it, so the
//! reply keeps showing it after the original changes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use themis_core::tools::{async_trait, ToolCallError, ToolRuntime, ToolT};

use super::AppState;
use crate::sink::EventSink;
use crate::types::{AttachedMedia, ThreadEvent, ThreadEventEnvelope};

/// Media one reply may carry.
pub const MAX_PER_REPLY: usize = 12;
/// Largest file shown inline.
const MAX_FILE_BYTES: u64 = 200 * 1024 * 1024;

pub(super) struct AttachFileTool {
    pub(super) state: AppState,
    pub(super) sink: Arc<dyn EventSink>,
    pub(super) thread_id: String,
    pub(super) run_id: String,
    pub(super) root: PathBuf,
    pub(super) attached: AtomicUsize,
}

impl std::fmt::Debug for AttachFileTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachFileTool").finish_non_exhaustive()
    }
}

fn error(message: impl ToString) -> ToolCallError {
    ToolCallError::RuntimeError(format!("attach_file: {}", message.to_string()).into())
}

/// `image`, `video` or `pdf` from a file's extension.
pub fn media_kind(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" => "image",
        "mp4" | "m4v" | "mov" | "webm" => "video",
        "pdf" => "pdf",
        _ => return None,
    })
}

/// The file `requested` names, inside `root`, copied into the project's
/// attachments storage unless it is already there.
fn stored_copy(root: &Path, requested: &str) -> Result<PathBuf, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let path = Path::new(requested);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let path = path
        .canonicalize()
        .map_err(|_| format!("{requested} does not exist"))?;
    if !path.starts_with(&root) {
        return Err(format!("{requested} is outside the project"));
    }
    let metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err(format!("{requested} is not a file"));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!("{requested} is larger than 200 MB"));
    }
    let storage = super::attachments::project_storage_directory(&root, &["attachments"])?;
    if path.starts_with(&storage) {
        return Ok(path);
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.chars().any(char::is_control))
        .ok_or("the file name is not valid")?;
    let folder = storage.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&folder).map_err(|e| e.to_string())?;
    let copy = folder.join(name);
    std::fs::copy(&path, &copy).map_err(|e| e.to_string())?;
    Ok(copy)
}

#[async_trait]
impl ToolRuntime for AttachFileTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let files = args["files"]
            .as_array()
            .filter(|files| !files.is_empty())
            .ok_or_else(|| error("give files: [{\"path\": ..., \"caption\": ...}]"))?;
        let already = self.attached.load(Ordering::SeqCst);
        if already + files.len() > MAX_PER_REPLY {
            return Err(error(format!(
                "a reply shows at most {MAX_PER_REPLY} files ({already} attached already); choose the ones that matter"
            )));
        }
        let mut media = Vec::new();
        for file in files {
            let requested = file["path"]
                .as_str()
                .or_else(|| file.as_str())
                .ok_or_else(|| error("each file needs a path"))?;
            let kind = media_kind(Path::new(requested)).ok_or_else(|| {
                error(format!(
                    "{requested}: attach_file shows images, videos and PDFs"
                ))
            })?;
            let caption = file["caption"]
                .as_str()
                .map(|caption| caption.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|caption| !caption.is_empty())
                .map(|caption| caption.chars().take(80).collect::<String>());
            let root = self.root.clone();
            let owned = requested.to_owned();
            let stored = tokio::task::spawn_blocking(move || stored_copy(&root, &owned))
                .await
                .map_err(error)?
                .map_err(error)?;
            if media
                .iter()
                .any(|existing: &AttachedMedia| existing.path == stored.to_string_lossy())
            {
                continue;
            }
            media.push(AttachedMedia {
                name: stored
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: stored.to_string_lossy().into_owned(),
                caption,
                kind: kind.to_owned(),
            });
        }
        self.attached.fetch_add(media.len(), Ordering::SeqCst);
        let envelope = ThreadEventEnvelope {
            thread_id: self.thread_id.clone(),
            run_id: self.run_id.clone(),
            event: ThreadEvent::MediaAttached {
                files: media.clone(),
            },
        };
        if let Err(problem) = self.state.inner.transcript.append_event(&envelope) {
            self.state.record_error("save_thread_event", problem);
        }
        self.sink.emit_thread_event(&envelope);
        Ok(json!({
            "attached": media.iter().map(|file| json!({"name": file.name, "kind": file.kind, "caption": file.caption})).collect::<Vec<_>>(),
            "note": "These appear under your reply. Do not repeat their paths in the text.",
        }))
    }
}

impl ToolT for AttachFileTool {
    fn name(&self) -> &str {
        "attach_file"
    }
    fn description(&self) -> &str {
        "Show images, videos or PDFs from the project inline under your reply, such as a browser_screenshot or browser_record result, a chart you made, or before/after examples. At most 12 per reply, in the order given; give each a short caption (\"Before\", \"After\"). Example: {\"files\":[{\"path\":\"/project/.themis/attachments/…/screenshot-1652-03.png\",\"caption\":\"Sign-up error\"}]}."
    }
    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "files": {
                    "type": "array", "minItems": 1, "maxItems": MAX_PER_REPLY,
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": {"type": "string", "description": "Project file (absolute or project-relative): png, jpg, gif, webp, mp4, webm, mov or pdf"},
                            "caption": {"type": "string", "description": "A few words shown under the tile"}
                        },
                        "required": ["path"], "additionalProperties": false
                    }
                }
            },
            "required": ["files"], "additionalProperties": false
        })
    }
    fn output_schema(&self) -> Option<Value> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_come_from_extensions() {
        assert_eq!(media_kind(Path::new("a/shot.PNG")), Some("image"));
        assert_eq!(media_kind(Path::new("clip.mp4")), Some("video"));
        assert_eq!(media_kind(Path::new("report.pdf")), Some("pdf"));
        assert_eq!(media_kind(Path::new("notes.txt")), None);
        assert_eq!(media_kind(Path::new("noextension")), None);
    }

    #[test]
    fn project_files_are_copied_into_storage_and_outside_files_refused() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("chart.png"), b"png").unwrap();
        let copy = stored_copy(project.path(), "chart.png").unwrap();
        assert!(copy.starts_with(
            project
                .path()
                .canonicalize()
                .unwrap()
                .join(".themis/attachments")
        ));
        assert_eq!(std::fs::read(&copy).unwrap(), b"png");
        // A file already in storage is used as it is.
        assert_eq!(
            stored_copy(project.path(), copy.to_str().unwrap()).unwrap(),
            copy
        );
        let outside = tempfile::NamedTempFile::new().unwrap();
        assert!(
            stored_copy(project.path(), outside.path().to_str().unwrap())
                .unwrap_err()
                .contains("outside the project")
        );
        assert!(stored_copy(project.path(), "../escape.png").is_err());
        assert!(stored_copy(project.path(), "missing.png")
            .unwrap_err()
            .contains("does not exist"));
    }
}
