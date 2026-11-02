//! Screenshots for models that read images.
//!
//! `browser_screenshot` saves a PNG for the person. When screenshots go to the model
//! (`browser.screenshots_to_model`), it also keeps a smaller copy here, keyed by the saved
//! file's path: at most [`MAX_SIDE`] pixels on its longest side and [`MAX_BYTES`] bytes.
//! The copies live in memory only, at most [`KEPT_COPIES`] of them, so neither transcripts,
//! checkpoints nor summaries ever hold image bytes.
//!
//! A run whose model reads images collects the copies its screenshots made into
//! [`RecentImages`]. Each model request then gets the latest [`KEPT_IMAGES`] of them, each as
//! a user message right after the tool results of the call that took it: OpenAI-compatible
//! APIs take no image in a tool-role message. An older screenshot (or one whose tool result
//! a checkpoint already folded away) goes back only as its tool result's text, so a long run
//! never resends every screenshot.

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use autoagents::llm::chat::{ChatMessage, ChatRole, ImageMime, MessageType};

/// Longest side of the copy a model sees, in pixels.
pub const MAX_SIDE: u32 = 1280;
/// Largest copy a model is sent; a larger screenshot goes as text only.
pub const MAX_BYTES: usize = 1_500_000;
/// Screenshots a request carries as images; older ones go as their text result.
pub const KEPT_IMAGES: usize = 2;
/// Model copies kept in memory, for the run and the goal check after it.
pub const KEPT_COPIES: usize = 8;

/// A screenshot as a model gets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub mime: ImageMime,
    pub bytes: Vec<u8>,
}

static COPIES: Mutex<VecDeque<(String, Image)>> = Mutex::new(VecDeque::new());

/// Keeps the model's copy of the screenshot saved at `path`; the oldest copy goes past
/// [`KEPT_COPIES`]. A copy over [`MAX_BYTES`] is not kept.
pub fn keep(path: &str, image: Image) -> bool {
    if image.bytes.is_empty() || image.bytes.len() > MAX_BYTES {
        return false;
    }
    let mut copies = COPIES.lock().unwrap_or_else(|e| e.into_inner());
    copies.retain(|(known, _)| known != path);
    copies.push_back((path.to_owned(), image));
    while copies.len() > KEPT_COPIES {
        copies.pop_front();
    }
    true
}

/// The model's copy of the screenshot saved at `path`, while it is still kept.
pub fn copy_of(path: &str) -> Option<Image> {
    COPIES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(known, _)| known == path)
        .map(|(_, image)| image.clone())
}

/// The path a `browser_screenshot` result says it attached for the model, if any.
pub fn attached_path(result: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(result).ok()?;
    if value["image_attached"] != true {
        return None;
    }
    value["path"].as_str().map(str::to_owned)
}

/// One screenshot a run's model may see: the tool call that took it and its copy.
struct Seen {
    call_id: String,
    note: String,
    image: Image,
}

/// The latest screenshots of a run whose model reads images, at most `keep`.
pub struct RecentImages {
    keep: usize,
    seen: VecDeque<Seen>,
}

/// What [`RecentImages::request`] put into one request, for diagnostics and calibration.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Attached {
    pub images: usize,
    pub bytes: usize,
}

impl RecentImages {
    pub fn new(keep: usize) -> Self {
        Self {
            keep,
            seen: VecDeque::new(),
        }
    }

    /// Adds the screenshot `call_id` took; the oldest goes past `keep`.
    pub fn push(&mut self, call_id: &str, note: String, image: Image) {
        if self.keep == 0 {
            return;
        }
        self.seen.push_back(Seen {
            call_id: call_id.to_owned(),
            note,
            image,
        });
        while self.seen.len() > self.keep {
            self.seen.pop_front();
        }
    }

    /// `messages` with each kept screenshot as a user message right after the tool results
    /// of its call. A screenshot whose results are no longer in `messages` is left out.
    /// Without screenshots `messages` is borrowed as is; otherwise one pass copies it.
    pub fn request<'a>(&self, messages: &'a [ChatMessage]) -> (Cow<'a, [ChatMessage]>, Attached) {
        if self.seen.is_empty() {
            return (Cow::Borrowed(messages), Attached::default());
        }
        let wanted: HashMap<&str, usize> = self
            .seen
            .iter()
            .enumerate()
            .map(|(index, seen)| (seen.call_id.as_str(), index))
            .collect();
        let mut out = Vec::with_capacity(messages.len() + self.seen.len());
        let mut attached = Attached::default();
        for message in messages {
            out.push(message.clone());
            let MessageType::ToolResult(results) = &message.message_type else {
                continue;
            };
            for result in results {
                let Some(&index) = wanted.get(result.id.as_str()) else {
                    continue;
                };
                let seen = &self.seen[index];
                attached.images += 1;
                attached.bytes += seen.image.bytes.len();
                out.push(ChatMessage {
                    role: ChatRole::User,
                    message_type: MessageType::Image((seen.image.mime, seen.image.bytes.clone())),
                    content: seen.note.clone(),
                });
            }
        }
        if attached.images == 0 {
            return (Cow::Borrowed(messages), attached);
        }
        (Cow::Owned(out), attached)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use autoagents::llm::{FunctionCall, ToolCall};

    fn call(id: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "browser_screenshot".into(),
                arguments: format!(r#"{{"path":"/p/{id}.png","image_attached":true}}"#),
            },
        }
    }

    /// A run that took `shots` screenshots, each between some ordinary work.
    fn run(shots: usize) -> (Vec<ChatMessage>, RecentImages) {
        let mut messages = vec![ChatMessage {
            role: ChatRole::User,
            message_type: MessageType::Text,
            content: "task".into(),
        }];
        let mut recent = RecentImages::new(KEPT_IMAGES);
        for index in 0..shots {
            let id = format!("shot-{index}");
            messages.push(ChatMessage {
                role: ChatRole::Assistant,
                message_type: MessageType::ToolUse(vec![call(&id)]),
                content: String::new(),
            });
            messages.push(ChatMessage {
                role: ChatRole::Tool,
                message_type: MessageType::ToolResult(vec![call(&id)]),
                content: String::new(),
            });
            recent.push(
                &id,
                format!("screenshot {index}"),
                Image {
                    mime: ImageMime::JPEG,
                    bytes: vec![index as u8; 10_000],
                },
            );
        }
        (messages, recent)
    }

    fn image_bytes(messages: &[ChatMessage]) -> usize {
        messages
            .iter()
            .map(|message| match &message.message_type {
                MessageType::Image((_, bytes)) => bytes.len(),
                _ => 0,
            })
            .sum()
    }

    #[test]
    fn each_kept_screenshot_follows_its_own_tool_result() {
        let (messages, recent) = run(3);
        let (request, attached) = recent.request(&messages);
        assert_eq!(
            attached,
            Attached {
                images: 2,
                bytes: 20_000
            }
        );
        // The first screenshot goes as its text result only; the later two follow theirs.
        let kinds: Vec<_> = request
            .iter()
            .map(|m| match &m.message_type {
                MessageType::Image(_) => m.content.clone(),
                MessageType::ToolResult(r) => r[0].id.clone(),
                _ => "-".into(),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "-",
                "-",
                "shot-0",
                "-",
                "shot-1",
                "screenshot 1",
                "-",
                "shot-2",
                "screenshot 2"
            ]
        );
        // The run's own history never holds an image.
        assert_eq!(image_bytes(&messages), 0);
    }

    #[test]
    fn a_screenshot_whose_result_was_folded_away_is_left_out() {
        let (messages, recent) = run(2);
        // A checkpoint replaced everything up to the last call.
        let (request, attached) = recent.request(&messages[3..]);
        assert_eq!(attached.images, 1);
        assert_eq!(request.len(), 3);
        let (request, attached) = recent.request(&messages[..1]);
        assert_eq!(attached, Attached::default());
        assert!(matches!(request, Cow::Borrowed(_)));
        let none = RecentImages::new(KEPT_IMAGES);
        assert!(matches!(none.request(&messages).0, Cow::Borrowed(_)));
        let mut off = RecentImages::new(0);
        off.push(
            "shot-0",
            String::new(),
            Image {
                mime: ImageMime::PNG,
                bytes: vec![1],
            },
        );
        assert_eq!(off.request(&messages).1, Attached::default());
    }

    /// Scale: a run with ten times the screenshots sends the same image bytes per request,
    /// and building the request does one pass over the history (messages copied grow with
    /// the history only).
    #[test]
    fn image_bytes_per_request_stay_bounded_as_screenshots_grow() {
        let (small, small_recent) = run(10);
        let (large, large_recent) = run(100);
        let (small_request, small_attached) = small_recent.request(&small);
        let (large_request, large_attached) = large_recent.request(&large);
        assert_eq!(small_attached.bytes, KEPT_IMAGES * 10_000);
        assert_eq!(large_attached.bytes, small_attached.bytes);
        assert_eq!(image_bytes(&large_request), image_bytes(&small_request));
        // Work counted: messages copied is history length plus the kept images.
        assert_eq!(small_request.len(), small.len() + KEPT_IMAGES);
        assert_eq!(large_request.len(), large.len() + KEPT_IMAGES);
        // Serialized request bytes: the large run's request grows by its text history only.
        let text = |messages: &[ChatMessage]| -> usize {
            messages
                .iter()
                .filter(|m| !matches!(m.message_type, MessageType::Image(_)))
                .map(|m| serde_json::to_vec(m).unwrap().len())
                .sum()
        };
        let small_total = text(&small_request) + image_bytes(&small_request);
        let large_total = text(&large_request) + image_bytes(&large_request);
        assert!(large_total - small_total == text(&large) - text(&small));
        // Safety limit: under one image per kept slot whatever the run's length.
        assert!(large_attached.images <= KEPT_IMAGES);
    }

    #[test]
    fn copies_are_bounded_and_found_by_path() {
        let shot = |byte: u8| Image {
            mime: ImageMime::JPEG,
            bytes: vec![byte; 4],
        };
        let base = format!("/tmp/vision-test-{}", std::process::id());
        for index in 0..KEPT_COPIES + 3 {
            assert!(keep(&format!("{base}/{index}.png"), shot(index as u8)));
        }
        assert_eq!(copy_of(&format!("{base}/0.png")), None, "oldest dropped");
        assert_eq!(
            copy_of(&format!("{base}/{}.png", KEPT_COPIES + 2)),
            Some(shot((KEPT_COPIES + 2) as u8))
        );
        assert!(!keep(
            &format!("{base}/big.png"),
            Image {
                mime: ImageMime::JPEG,
                bytes: vec![0; MAX_BYTES + 1],
            }
        ));
        assert!(!keep(
            &format!("{base}/empty.png"),
            Image {
                mime: ImageMime::JPEG,
                bytes: Vec::new(),
            }
        ));
        assert_eq!(
            attached_path(r#"{"path":"/a.png","image_attached":true}"#).as_deref(),
            Some("/a.png")
        );
        assert_eq!(attached_path(r#"{"path":"/a.png"}"#), None);
        assert_eq!(attached_path("not json"), None);
    }
}
