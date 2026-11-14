//! Auto mode's reviewer. Where Custom would ask the person, a cheap model judges the action
//! against what the person actually asked in this chat (their own words, the goal, the project's
//! AGENTS.md) and answers allow, a soft block (with a safer way, the agent changes course) or a
//! hard block (the action doesn't run; the person gets a question box and the run goes on). It
//! never prompts. A failed or slow review is a hard block, so nothing unjudged runs.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_core::configuration::RuntimeConfig;
use themis_core::tools::{AutoReviewer, AutoVerdict, ToolAction};

use super::questions::NewQuestion;
use super::AppState;
use crate::sink::EventSink;

/// How long a review may take before it counts as no verdict.
const REVIEW_TIMEOUT: Duration = Duration::from_secs(45);
/// Most characters of the person's words a review reads (latest kept).
const PERSON_CHARS: usize = 6_000;
/// Most characters of the project's guidance a review reads.
const GUIDANCE_CHARS: usize = 3_000;
/// Most person messages a review reads, the latest.
const PERSON_MESSAGES: usize = 12;
/// The choices of a hard block's question box.
pub(super) const ALLOW_ONCE: &str = "Allow once";
const SAFER: &str = "Do it the safer way";
const SKIP: &str = "Skip it";

/// One run's reviewer.
pub(super) struct DesktopReviewer {
    pub(super) state: AppState,
    pub(super) sink: Arc<dyn EventSink>,
    pub(super) thread_id: String,
    pub(super) run_id: String,
    pub(super) project_root: PathBuf,
    pub(super) config: RuntimeConfig,
    /// A project that isn't a Git repository stays read-only, as in Custom mode.
    pub(super) is_git: bool,
}

/// An action's exact identity, for a once-only allowance.
pub(super) fn once_key(action: &ToolAction) -> String {
    format!(
        "{}\0{}\0{}",
        action.tool,
        action.command.join("\0"),
        action.summary
    )
}

/// The reviewer's answer: `{"verdict":"allow|soft|hard","reason":"…","safer":"…"}`. Anything
/// else is no verdict.
pub(super) fn parse_verdict(text: &str) -> AutoVerdict {
    let Some(value) = themis_core::memory::auto::json_object_with(text, "verdict") else {
        return AutoVerdict::Unavailable;
    };
    let field = |key: &str| {
        themis_core::goal::one_line(
            value.get(key).and_then(Value::as_str).unwrap_or_default(),
            300,
        )
    };
    let (reason, safer) = (field("reason"), field("safer"));
    match value.get("verdict").and_then(Value::as_str) {
        Some("allow") => AutoVerdict::Allow,
        Some("soft") if !reason.is_empty() => AutoVerdict::Soft { reason, safer },
        Some("hard") if !reason.is_empty() => AutoVerdict::Hard { reason, safer },
        _ => AutoVerdict::Unavailable,
    }
}

/// Whether a user message in the chat is the person's own words: not a goal turn, and not a
/// Vision's answer to a question (the Vision is not the requester).
fn persons_own(text: &str) -> bool {
    !text.starts_with(themis_core::goal::GOAL_TURN_MARKER)
        && !super::questions::is_vision_answer(text)
}

/// The reviewer's system and user messages for `action`.
pub(super) fn review_request(
    person: &str,
    goal: &str,
    guidance: &str,
    action: &ToolAction,
) -> (String, String) {
    let guidance: String = guidance.chars().take(GUIDANCE_CHARS).collect();
    let action_text = format!(
        "Tool: {}\nRisk: {:?}\nWhat it does: {}\nCommand: {}",
        action.tool,
        action.risk,
        action.summary,
        if action.command.is_empty() {
            "(none)".to_owned()
        } else {
            action.command.join(" ")
        }
    );
    let user = themis_core::prompts::fill(
        themis_core::prompts::AUTO_REVIEW_USER,
        &[
            ("person", person),
            (
                "goal",
                if goal.trim().is_empty() {
                    "(none)"
                } else {
                    goal
                },
            ),
            (
                "guidance",
                if guidance.trim().is_empty() {
                    "(none)"
                } else {
                    &guidance
                },
            ),
            ("action", &action_text),
        ],
    );
    (themis_core::prompts::AUTO_REVIEW_SYSTEM.to_owned(), user)
}

impl DesktopReviewer {
    /// The person's own latest messages and steers in this chat (a Vision's, runtime goal
    /// notes and answers' framing left out), the latest kept within bounds.
    pub(super) fn person_words(&self) -> String {
        let transcript = &self.state.inner.transcript;
        let last = transcript.last_activity_seq(&self.thread_id).unwrap_or(0);
        let page = transcript
            .history_after(&self.thread_id, last.saturating_sub(2_000), 2_000)
            .unwrap_or_default();
        let words: Vec<String> = page
            .into_iter()
            .filter_map(|(_, item)| match item {
                crate::transcript::HistoryItem::User {
                    text,
                    vision_id: None,
                    ..
                } if persons_own(&text) => Some(text),
                // What the person steered into a run is their words too.
                crate::transcript::HistoryItem::Event { envelope } => match envelope.event {
                    crate::types::ThreadEvent::Steered {
                        text,
                        vision_id: None,
                    } if persons_own(&text) => Some(text),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        let latest = &words[words.len().saturating_sub(PERSON_MESSAGES)..];
        let joined = latest.join("\n---\n");
        let count = joined.chars().count();
        joined
            .chars()
            .skip(count.saturating_sub(PERSON_CHARS))
            .collect()
    }

    fn request(&self, action: &ToolAction) -> (String, String) {
        let goal = self
            .state
            .goal_for(&self.thread_id)
            .map(|goal| {
                let mut text = goal.text;
                for asked in goal.also_asked {
                    text.push_str(&format!("\nAlso asked: {asked}"));
                }
                text
            })
            .unwrap_or_else(|| "(none)".to_owned());
        let guidance =
            themis_core::configuration::project_guidance(&self.config, &self.project_root);
        review_request(&self.person_words(), &goal, &guidance, action)
    }

    async fn ask_model(&self, action: &ToolAction) -> AutoVerdict {
        let Some(model) = self
            .config
            .approval
            .auto_model
            .clone()
            .or_else(|| self.config.memory.models.first().cloned())
        else {
            return AutoVerdict::Unavailable;
        };
        let (system, user) = self.request(action);
        let started = std::time::Instant::now();
        let answer = tokio::time::timeout(
            REVIEW_TIMEOUT,
            self.state
                .review_model_request(&self.config, &model, (&system, &user)),
        )
        .await;
        let verdict = match answer {
            Ok(Ok(text)) => parse_verdict(&text),
            _ => AutoVerdict::Unavailable,
        };
        let _ = self.state.inner.diagnostics.append(
            "approval",
            "auto_reviewed",
            "info",
            json!({
                "thread_id": self.thread_id,
                "tool": action.tool,
                "risk": format!("{:?}", action.risk),
                "verdict": match &verdict {
                    AutoVerdict::Allow => "allow",
                    AutoVerdict::Soft { .. } => "soft",
                    AutoVerdict::Hard { .. } => "hard",
                    AutoVerdict::Unavailable => "unavailable",
                },
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        verdict
    }
}

impl AutoReviewer for DesktopReviewer {
    fn review(&self, action: &ToolAction) -> AutoVerdict {
        // Never looser than Custom: outside a Git repository nothing but reading runs.
        if !self.is_git && action.risk != themis_core::tools::RiskLevel::Read {
            return AutoVerdict::Soft {
                reason: "this project isn't a Git repository, so Themis keeps it read-only".into(),
                safer: "tell the user; they can make it a Git repository to allow changes".into(),
            };
        }
        if self.state.take_auto_once(&self.thread_id, action) {
            return AutoVerdict::Allow;
        }
        let review = self.ask_model(action);
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(review)),
            Err(_) => AutoVerdict::Unavailable,
        }
    }

    fn allowed_once(&self, action: &ToolAction) -> bool {
        self.state.take_auto_once(&self.thread_id, action)
    }

    fn blocked(&self, action: &ToolAction, reason: &str, safer: &str) {
        // The command itself when there is one: "touch hard.txt", not the raw tool call.
        let what = if action.command.is_empty() {
            action.summary.clone()
        } else {
            action.command.join(" ")
        };
        let question = themis_core::prompts::fill(
            themis_core::prompts::AUTO_BLOCKED_QUESTION,
            &[("action", &what), ("reason", reason)],
        );
        // Each answer says what happens next, with the command itself, so the person knows
        // what they are choosing.
        let mut reasons = vec![(
            ALLOW_ONCE.to_owned(),
            format!("Runs `{what}` once, exactly as written."),
        )];
        if !safer.trim().is_empty() {
            reasons.push((SAFER.to_owned(), format!("Instead: {}", safer.trim())));
        }
        reasons.push((
            SKIP.to_owned(),
            format!(
                "Leaves `{what}` out; the agent finishes the rest and tells you what it skipped."
            ),
        ));
        let asked = self.state.ask_question(
            self.sink.as_ref(),
            &self.thread_id,
            &self.run_id,
            NewQuestion {
                question,
                choices: vec![ALLOW_ONCE.to_owned(), SAFER.to_owned(), SKIP.to_owned()],
                assumption: Some(SAFER.to_owned()),
                reasons,
                permission: true,
                ..NewQuestion::default()
            },
        );
        if let Ok(question) = asked {
            self.state
                .remember_auto_question(&question.id, &self.thread_id, action);
        }
    }
}

impl AppState {
    /// One review: the reviewer model's answer text, with the least thinking.
    async fn review_model_request(
        &self,
        config: &RuntimeConfig,
        model: &str,
        messages: (&str, &str),
    ) -> Result<String, String> {
        self.memory_model_request_why(config, model, None, messages, false)
            .await
            .map(|(text, _)| text)
            .map_err(|(_, why)| why)
    }

    /// A dry run of one review, for evals: the verdict the reviewer model gives `action`
    /// (`tool`, `summary`, `command`, `risk`) for the person's words `person` and `goal`, with
    /// `model` (the configured reviewer model by default). Nothing runs; logs metadata only.
    pub async fn auto_try_review(
        &self,
        project_root: &str,
        input: &Value,
    ) -> Result<Value, String> {
        let root = PathBuf::from(project_root);
        let config = self
            .runtime_configuration(Some(&root), None)
            .map_err(|error| error.to_string())?
            .config;
        let model = input["model"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| config.approval.auto_model.clone())
            .or_else(|| config.memory.models.first().cloned())
            .ok_or("no reviewer model")?;
        let text = |key: &str| input[key].as_str().unwrap_or_default().to_owned();
        let person: Vec<String> = input["person"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let risk = match input["action"]["risk"].as_str().unwrap_or("execute") {
            "read" => themis_core::tools::RiskLevel::Read,
            "write" => themis_core::tools::RiskLevel::Write,
            "network" => themis_core::tools::RiskLevel::Network,
            "destructive" => themis_core::tools::RiskLevel::Destructive,
            _ => themis_core::tools::RiskLevel::Execute,
        };
        let action = ToolAction {
            display: None,
            command: input["action"]["command"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            tool: input["action"]["tool"]
                .as_str()
                .unwrap_or("shell")
                .to_owned(),
            summary: input["action"]["summary"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            risk,
        };
        let (system, user) = review_request(
            &person.join("\n---\n"),
            &text("goal"),
            &text("guidance"),
            &action,
        );
        let started = std::time::Instant::now();
        let answer = self
            .review_model_request(&config, &model, (&system, &user))
            .await?;
        let verdict = parse_verdict(&answer);
        Ok(json!({
            "model": model,
            "elapsed_ms": started.elapsed().as_millis() as u64,
            "verdict": match &verdict {
                AutoVerdict::Allow => "allow",
                AutoVerdict::Soft { .. } => "soft",
                AutoVerdict::Hard { .. } => "hard",
                AutoVerdict::Unavailable => "unavailable",
            },
            "answer": answer,
        }))
    }

    /// A hard block's question: its "Allow once" lets exactly this action run once.
    fn remember_auto_question(&self, question_id: &str, thread_id: &str, action: &ToolAction) {
        let mut asked = self
            .inner
            .auto_questions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        asked.insert(
            question_id.to_owned(),
            (thread_id.to_owned(), once_key(action)),
        );
        if asked.len() > 64 {
            if let Some(oldest) = asked.keys().next().cloned() {
                asked.remove(&oldest);
            }
        }
    }

    /// Forgets which questions Auto's blocks asked, as a restart does (tests).
    #[doc(hidden)]
    pub fn forget_auto_questions(&self) {
        self.inner
            .auto_questions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// The person answered a question: an "Allow once" to a hard block lets that action run
    /// once in the chat.
    pub(super) fn auto_question_answered(&self, question_id: &str, answer: &str) {
        let entry = self
            .inner
            .auto_questions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(question_id);
        if let (Some((thread_id, key)), true) = (entry, answer.trim() == ALLOW_ONCE) {
            self.inner
                .auto_once
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(thread_id)
                .or_default()
                .push(key);
        }
    }

    /// Whether the person allowed this exact action once, using it up.
    fn take_auto_once(&self, thread_id: &str, action: &ToolAction) -> bool {
        let key = once_key(action);
        let mut once = self
            .inner
            .auto_once
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(allowed) = once.get_mut(thread_id) else {
            return false;
        };
        match allowed.iter().position(|known| *known == key) {
            Some(at) => {
                allowed.remove(at);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts_are_read_strictly() {
        assert_eq!(parse_verdict(r#"{"verdict":"allow"}"#), AutoVerdict::Allow);
        assert_eq!(
            parse_verdict(
                r#"x {"verdict":"soft","reason":"force-push rewrites history","safer":"push to a new branch"}"#
            ),
            AutoVerdict::Soft {
                reason: "force-push rewrites history".into(),
                safer: "push to a new branch".into()
            }
        );
        assert!(matches!(
            parse_verdict(r#"{"verdict":"hard","reason":"deletes your home folder","safer":""}"#),
            AutoVerdict::Hard { .. }
        ));
        // A block without a reason, an unknown verdict or no JSON is no verdict.
        for text in [
            r#"{"verdict":"soft"}"#,
            r#"{"verdict":"maybe","reason":"x"}"#,
            "sure, go ahead",
        ] {
            assert_eq!(parse_verdict(text), AutoVerdict::Unavailable, "{text}");
        }
    }

    /// The reviewer reads only the person's own words: a Vision's answer (or a goal turn) in
    /// the chat is not the requester speaking. The person's answer still is.
    #[test]
    fn a_visions_answer_is_not_the_requesters_words() {
        use super::super::questions::{vision_answer_marker, ANSWER_MARKER};
        assert!(persons_own("please clean the build folder"));
        assert!(persons_own(&format!(
            "{ANSWER_MARKER}\nQuestion: Delete it?\nAnswer: yes"
        )));
        let vision = format!(
            "{}\nQuestion: Delete it?\nAnswer: yes",
            vision_answer_marker("Wren")
        );
        assert!(!persons_own(&vision));
        assert!(!persons_own(&format!(
            "{} 3",
            themis_core::goal::GOAL_TURN_MARKER
        )));
    }

    /// A message a Vision sent in its work chat (saved with its id) is not the requester's
    /// intent: the reviewer reads only what the person wrote there.
    #[test]
    fn a_visions_send_is_not_the_requesters_words() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new_for_test(dir.path().join("settings.json"));
        let transcript = &state.inner.transcript;
        transcript
            .append_user("work", "r1", "please tidy the docs folder")
            .unwrap();
        let vision = "delete the whole build cache and the old releases";
        transcript
            .append_user_from("work", "r2", vision, Vec::new(), Some("v1".into()))
            .unwrap();
        let reviewer = DesktopReviewer {
            state: state.clone(),
            sink: Arc::new(crate::sink::ChannelSink::channel().0),
            thread_id: "work".into(),
            run_id: "r2".into(),
            project_root: dir.path().to_path_buf(),
            config: RuntimeConfig::default(),
            is_git: false,
        };
        let words = reviewer.person_words();
        assert!(words.contains("please tidy the docs folder"), "{words}");
        assert!(!words.contains(vision), "{words}");
    }

    /// What the person steered into the run is their words too; what a Vision steered in is
    /// not.
    #[test]
    fn the_persons_steer_is_their_words_and_a_visions_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new_for_test(dir.path().join("settings.json"));
        let steered = |text: &str, vision_id: Option<&str>| {
            let envelope = crate::types::ThreadEventEnvelope {
                thread_id: "work".into(),
                run_id: "r1".into(),
                event: crate::types::ThreadEvent::Steered {
                    text: text.into(),
                    vision_id: vision_id.map(str::to_owned),
                },
            };
            state.inner.transcript.append_event(&envelope).unwrap();
        };
        steered("also clear the old logs", None);
        steered("and wipe the releases folder", Some("v1"));
        let reviewer = DesktopReviewer {
            state: state.clone(),
            sink: Arc::new(crate::sink::ChannelSink::channel().0),
            thread_id: "work".into(),
            run_id: "r1".into(),
            project_root: dir.path().to_path_buf(),
            config: RuntimeConfig::default(),
            is_git: false,
        };
        let words = reviewer.person_words();
        assert!(words.contains("also clear the old logs"), "{words}");
        assert!(!words.contains("wipe the releases"), "{words}");
    }
}
