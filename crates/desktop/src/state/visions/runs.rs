//! A Vision's run: a turn of its hidden Vision chat for one channel, started by a wake.
//!
//! The run is an ordinary chat run with these differences, captured when it starts: its system
//! head is the Vision's (`visions-system.md`: who it is, its channel, the `themis` calls that
//! drive chats and its autonomy table) in place of the ordinary instructions; it has only
//! `read_file`, `list_dir`, `search_file` and the shell (for `themis` only), on every OS, and no
//! plugins, skills, subagents, browser or memory; it runs in a fixed envelope whatever the channel's mode
//! (`envelope`: Custom's rules, plain `themis` without asking, never a box; the sandbox's
//! `vision_envelope`: read-only, no network but the app server, no Keychain or secrets), and
//! is refused, with a notice in the channel, where no sandbox can hold it; and its credential
//! carries the Vision grant (`RunScope.vision`), so its `themis` calls may drive the chats the
//! Vision owns. Its final reply is posted in the channel as the Vision's message.

use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use themis_core::agent_cli::{RunScope, VisionScope};
use themis_core::configuration::{ApprovalMode, ApprovalPolicy, VisionsConfig};
use themis_core::prompts::{fill, VISIONS_READ_ONLY_NOTE, VISIONS_SYSTEM, VISIONS_WAKE_TURN};
use themis_core::sandbox::SandboxPlan;
use themis_core::tools::{ApprovalHook, ToolT};
use themis_core::visions::{profile_decides, Decision};

use super::{Channel, ChannelEntry, Hold, NoticeKind, OwnedKind, Owner, Vision, WakeReason};
use crate::sink::EventSink;
use crate::state::queue::ServerSend;
use crate::state::{AppState, RunSnapshot};
use crate::types::{ProviderKind, ThreadEvent};

/// Bytes of the channel's newest entries a wake message carries.
const RECENT_BYTES: usize = 8 * 1024;
/// Longest Vision message posted in a channel, in characters; longer replies are cut.
const MAX_REPLY_CHARS: usize = 4000;

/// Which Vision and channel a run of a Vision chat is for (`RunSnapshot.vision`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VisionRunSpec {
    pub(crate) vision_id: String,
    pub(crate) channel_id: String,
}

/// What a Vision run gets beyond an ordinary run, captured when it starts.
pub(in crate::state) struct VisionRun {
    /// Its system head, in place of the ordinary instructions.
    head: String,
    /// Its grant: the Vision, the channel and the channel's project.
    scope: VisionScope,
    /// The app server's port, the only network its commands reach.
    port: u16,
}

/// Why a wake's run did not start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotStarted {
    /// Refused before it ran: its reasons wait; tried again on a run's end only when `busy`.
    Refused(&'static str),
    /// Nothing here can confine it: it never runs, and the channel says so.
    Unconfinable,
}

/// A Vision run's sandbox: its fixed envelope (see `SandboxPlan::vision_envelope`), or one
/// refusing every command when it cannot be built after all. Other runs keep theirs.
pub(in crate::state) fn confine(
    vision: Option<&VisionRun>,
    sandbox: Arc<SandboxPlan>,
    root: &Path,
) -> Arc<SandboxPlan> {
    match vision {
        Some(vision) => Arc::new(
            sandbox
                .vision_envelope(root, vision.port)
                .unwrap_or_else(|_| {
                    sandbox.fail_closed(themis_core::prompts::VISIONS_SANDBOX_REFUSED.to_owned())
                }),
        ),
        None => sandbox,
    }
}

/// A Vision run's approvals: its fixed envelope (see `envelope`), audited like any run's.
/// Other runs keep theirs.
pub(in crate::state) fn vision_approvals(
    vision: Option<&VisionRun>,
    approvals: Arc<dyn ApprovalHook>,
    policy: &ApprovalPolicy,
    root: &Path,
) -> Arc<dyn ApprovalHook> {
    match vision {
        Some(_) => super::envelope::envelope_approvals(policy, root),
        None => approvals,
    }
}

/// The tools of a Vision run, on every OS: the read tools, and the shell (for `themis` only).
const VISION_TOOLS: [&str; 4] = ["read_file", "list_dir", "search_file", "shell"];

/// A Vision run's tools: [`VISION_TOOLS`], with `list_dir` and `search_file` added where the
/// run's own set lists and searches through the shell (Unix), gated by its `approvals` and
/// `sandbox`. Other runs keep theirs.
pub(in crate::state) fn run_tools(
    vision: Option<&VisionRun>,
    mut tools: Vec<Box<dyn ToolT>>,
    root: &Path,
    approvals: &Arc<dyn ApprovalHook>,
    sandbox: &Arc<SandboxPlan>,
) -> Vec<Box<dyn ToolT>> {
    if vision.is_none() {
        return tools;
    }
    tools.retain(|tool| VISION_TOOLS.contains(&tool.name()));
    if !tools.iter().any(|tool| tool.name() == "list_dir") {
        // Without them the run could read only paths it already knew.
        match themis_core::tools::listing_tools(root, Arc::clone(approvals), Some(sandbox)) {
            Ok(listing) => tools.extend(listing),
            Err(_) => themis_core::diagnostics::emit(
                "visions",
                "read_tools_unavailable",
                "warn",
                json!({"vision_id": vision.map(|run| run.scope.vision_id.clone())}),
            ),
        }
    }
    tools
}

/// A run's credential scope; a Vision run's also carries its grant.
pub(in crate::state) fn run_scope(mut scope: RunScope, vision: Option<&VisionRun>) -> RunScope {
    scope.vision = vision.map(|vision| vision.scope.clone());
    scope
}

/// The run's future, boxed; a Vision run's with its system head in place of the ordinary
/// instructions.
pub(in crate::state) fn boxed_run<'a, F>(
    vision: Option<VisionRun>,
    future: F,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = F::Output> + Send + 'a>>
where
    F: std::future::Future + Send + 'a,
{
    match vision {
        None => Box::pin(future),
        Some(vision) => Box::pin(themis_core::runtime::with_system_head(vision.head, future)),
    }
}

/// The Vision's system head for a run in `channel`. Its profile's autonomy and rows may only
/// lower the configured ones (`profile_decides`).
pub(super) fn vision_head(vision: &Vision, channel: &Channel, config: &VisionsConfig) -> String {
    let table = Decision::ALL
        .into_iter()
        .map(|decision| {
            let decides = profile_decides(
                config.autonomy,
                &config.custom,
                vision.autonomy,
                vision.custom.as_ref(),
                decision,
            );
            let who = if decides {
                "you decide"
            } else {
                "ask the person"
            };
            format!("- `{}`: {who}", decision.id())
        })
        .collect::<Vec<_>>()
        .join("\n");
    // The project goes into JSON the Vision copies, so it is written as JSON writes it.
    let project =
        serde_json::to_string(&channel.project_root.to_string_lossy()).unwrap_or_default();
    let project = project.trim_matches('"');
    let or_dash = |text: &str| {
        let text = text.trim();
        if text.is_empty() {
            "-".to_owned()
        } else {
            text.to_owned()
        }
    };
    let head = fill(
        VISIONS_SYSTEM,
        &[
            ("name", &vision.name),
            ("role", &or_dash(&vision.role)),
            ("soul", &or_dash(&vision.soul)),
            ("channel", &channel.name),
            ("channel_id", &channel.id),
            ("project", project),
            ("autonomy_table", &table),
        ],
    );
    format!("{head}\n\n{VISIONS_READ_ONLY_NOTE}")
}

/// Whether a failed send never reaches the run's end step, which would end the wake: refused
/// before it was saved, or saved but never started (its start could not be recorded).
fn never_ran(progress: ServerSend) -> bool {
    matches!(progress, ServerSend::Pending | ServerSend::Saved)
}

/// Why a failed send (`error`) did not start the Vision's run, when it never ran: `busy` (the
/// chat or the app's run limit, or a start not recorded), retried when a run ends, or
/// `not_started` (the run's own setup refused it: the Vision paused or gone, Visions off, no
/// sandbox), which waits for the next wake. `None` when the run's end step takes over.
fn start_refusal(error: &str, progress: ServerSend) -> Option<&'static str> {
    if !never_ran(progress) {
        return None;
    }
    let busy = progress == ServerSend::Saved || crate::state::threads::send_must_wait(error);
    Some(if busy { "busy" } else { "not_started" })
}

/// `text` cut to [`MAX_REPLY_CHARS`] characters.
fn bounded(text: &str) -> String {
    match text.char_indices().nth(MAX_REPLY_CHARS) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

impl AppState {
    /// The Vision and channel of `thread_id` when it is a Vision chat.
    pub(in crate::state) fn visions_run_spec(&self, thread_id: &str) -> Option<VisionRunSpec> {
        self.owner_of(thread_id)
            .filter(|owner| owner.kind == OwnedKind::VisionChat)
            .map(|owner| VisionRunSpec {
                vision_id: owner.vision_id,
                channel_id: owner.channel_id,
            })
    }

    /// Captures a Vision chat's run: its head, grant and envelope (Custom's rules whatever the
    /// channel's mode, refused where no sandbox can hold it), and a configuration without
    /// plugins, skills, subagents, browser or memory. Any other run passes through unchanged.
    pub(in crate::state) fn visions_run_setup(
        &self,
        snapshot: &mut RunSnapshot,
    ) -> Result<Option<VisionRun>, String> {
        let Some(spec) = snapshot.vision.clone() else {
            return Ok(None);
        };
        let vision = self
            .vision_get(&spec.vision_id)
            .ok_or_else(|| "This Vision no longer exists".to_owned())?;
        if vision.paused {
            return Err("This Vision is paused".to_owned());
        }
        let channel = self
            .channel_get(&spec.channel_id)
            .ok_or_else(|| "This channel no longer exists".to_owned())?;
        let port =
            self.visions_envelope_check(&snapshot.work_root, &snapshot.runtime_configuration)?;
        snapshot.approval_mode = ApprovalMode::Custom;
        let config = &mut snapshot.runtime_configuration;
        let head = vision_head(&vision, &channel, &config.visions);
        config.subagents.enabled = false;
        config.browser.enabled = false;
        config.memory.enabled = false;
        snapshot.plugins.clear();
        snapshot.skills.clear();
        snapshot.skill_catalog.clear();
        let scope = VisionScope {
            vision_id: spec.vision_id,
            channel_id: spec.channel_id,
            project_root: channel.project_root,
        };
        Ok(Some(VisionRun { head, scope, port }))
    }

    /// Whether a Vision run in `root` under `config` can be held in its envelope; the app
    /// server's port when it can, why not when it cannot.
    pub(in crate::state) fn visions_envelope_check(
        &self,
        root: &Path,
        config: &themis_core::configuration::RuntimeConfig,
    ) -> Result<u16, String> {
        let port = themis_core::agent_cli::port()
            .ok_or_else(|| "No Themis app server runs for a Vision's `themis` calls".to_owned())?;
        self.sandbox_plan(root, &config.sandbox, None)
            .vision_envelope(root, port)
            .map(|_| port)
    }

    /// The model a Vision's chats run on: its profile's, else the app's default.
    pub(super) async fn visions_model(&self, vision: &Vision) -> String {
        match vision
            .model
            .clone()
            .filter(|model| !model.trim().is_empty())
        {
            Some(model) => model,
            None => self.inner.settings.get().await.default_model,
        }
    }

    /// Starts the Vision's run on its own task (see `spawn_after_run` for why it is boxed).
    pub(super) fn visions_spawn_run(
        &self,
        sink: &Arc<dyn EventSink>,
        channel_id: &str,
        vision_id: &str,
        reasons: Vec<WakeReason>,
    ) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            // Nothing runs outside the app's runtime: the reasons wait for the next wake.
            self.visions_cache()
                .wakes
                .refused(vision_id, channel_id, reasons, false);
            self.visions_note(
                "run_refused",
                json!({"vision_id": vision_id, "channel_id": channel_id, "reason": "no_runtime"}),
            );
            return;
        };
        let state = self.clone();
        let sink = Arc::clone(sink);
        let (channel_id, vision_id) = (channel_id.to_owned(), vision_id.to_owned());
        let task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
            Box::pin(async move {
                state
                    .visions_run(sink, channel_id, vision_id, reasons)
                    .await
            });
        runtime.spawn(task);
    }

    /// Sends the wake message in the Vision chat. Refused before it was saved, the reasons
    /// wait again and the wake does not count; once saved, the run's end step takes over.
    async fn visions_run(
        &self,
        sink: Arc<dyn EventSink>,
        channel_id: String,
        vision_id: String,
        reasons: Vec<WakeReason>,
    ) {
        // Boxed, like each step below: a run's futures are large, and a debug build's worker
        // stack holds every one an `await` builds in place.
        let started =
            Box::pin(self.visions_run_start(sink, &channel_id, &vision_id, &reasons)).await;
        match started {
            Ok((thread_id, run_id)) => self.visions_note(
                "run_started",
                json!({"vision_id": vision_id, "channel_id": channel_id, "thread_id": thread_id, "run_id": run_id, "reasons": reasons.len()}),
            ),
            Err(NotStarted::Refused(why)) => {
                // Only a busy chat or run limit clears by itself; anything else waits for the
                // next wake rather than being tried on every run end.
                let retry = why == "busy";
                self.visions_cache()
                    .wakes
                    .refused(&vision_id, &channel_id, reasons, retry);
                self.visions_note(
                    "run_refused",
                    json!({"vision_id": vision_id, "channel_id": channel_id, "reason": why}),
                );
            }
            // Counted and dropped: tried again only when something wakes it anew.
            Err(NotStarted::Unconfinable) => {
                self.visions_cache().wakes.ended(&vision_id, &channel_id);
                let _ = self.channel_append(
                    &channel_id,
                    ChannelEntry::Notice {
                        kind: NoticeKind::SandboxUnavailable,
                        vision_id: Some(vision_id.clone()),
                        thread_id: None,
                    },
                );
                self.visions_note(
                    "run_refused",
                    json!({"vision_id": vision_id, "channel_id": channel_id, "reason": "sandbox_unavailable"}),
                );
            }
        }
    }

    async fn visions_run_start(
        &self,
        sink: Arc<dyn EventSink>,
        channel_id: &str,
        vision_id: &str,
        reasons: &[WakeReason],
    ) -> Result<(String, String), NotStarted> {
        let gone = NotStarted::Refused("gone");
        let channel = self.channel_get(channel_id).ok_or(gone)?;
        let vision = self.vision_get(vision_id).ok_or(gone)?;
        let config = self
            .runtime_configuration(Some(&channel.project_root), None)
            .map_err(|_| NotStarted::Refused("configuration"))?
            .config;
        if self
            .visions_envelope_check(&channel.project_root, &config)
            .is_err()
        {
            return Err(NotStarted::Unconfinable);
        }
        let chat = Box::pin(self.visions_chat(&vision, &channel))
            .await
            .map_err(|_| NotStarted::Refused("no_chat"))?;
        let text = self.visions_wake_text(channel_id, reasons);
        let run_id = uuid::Uuid::new_v4().to_string();
        let (sent, progress) =
            Box::pin(self.send_from_server(sink, &chat, text, &run_id, None, Vec::new())).await;
        if let Some(why) = sent.err().and_then(|error| start_refusal(&error, progress)) {
            return Err(NotStarted::Refused(why));
        }
        Ok((chat, run_id))
    }

    /// The Vision's own chat for `channel`, made on its first wake: hidden from the plain
    /// list, titled "{name} · #{channel}", on the Vision's model, in Custom (its runs keep their
    /// own envelope whatever the channel's mode).
    async fn visions_chat(&self, vision: &Vision, channel: &Channel) -> Result<String, String> {
        let existing = self.owners_where(|owner| {
            owner.vision_id == vision.id
                && owner.channel_id == channel.id
                && owner.kind == OwnedKind::VisionChat
        });
        for owner in existing {
            if self.require_thread(&owner.thread_id).await.is_ok() {
                return Ok(owner.thread_id);
            }
            // Its chat is gone (deleted by hand): a new one replaces it.
            self.owner_release(&owner.thread_id)?;
        }
        let model = self.visions_model(vision).await;
        let info = Box::pin(self.create_thread(
            channel.project_root.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some(model),
        ))
        .await?;
        let adopted = Box::pin(self.visions_adopt_chat(vision, channel, &info.id)).await;
        if let Err(error) = adopted {
            let _ = Box::pin(self.discard_thread(info.id.clone())).await;
            return Err(error);
        }
        self.visions_note(
            "vision_chat_created",
            json!({"vision_id": vision.id, "channel_id": channel.id, "thread_id": info.id}),
        );
        Ok(info.id)
    }

    async fn visions_adopt_chat(
        &self,
        vision: &Vision,
        channel: &Channel,
        thread_id: &str,
    ) -> Result<(), String> {
        // The title is only a label: a name too long for one keeps the chat untitled.
        let title = format!("{} · #{}", vision.name, channel.name);
        let _ = self.rename_thread(thread_id.to_owned(), title).await;
        self.set_thread_approval_mode(thread_id.to_owned(), ApprovalMode::Custom)
            .await?;
        self.owner_set(Owner {
            thread_id: thread_id.to_owned(),
            vision_id: vision.id.clone(),
            channel_id: channel.id.clone(),
            kind: OwnedKind::VisionChat,
            hold: Hold::None,
        })
    }

    /// The wake message: why it woke and the channel's newest entries.
    pub(super) fn visions_wake_text(&self, channel_id: &str, reasons: &[WakeReason]) -> String {
        let reasons = reasons
            .iter()
            .map(WakeReason::line)
            .collect::<Vec<_>>()
            .join("\n");
        let recent = self
            .channel_page(channel_id, None, RECENT_BYTES)
            .map(|page| {
                page.entries
                    .iter()
                    .filter_map(|line| serde_json::to_string(&line.entry).ok())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        fill(
            VISIONS_WAKE_TURN,
            &[("reasons", &reasons), ("recent", &recent)],
        )
    }

    /// Posts a Vision chat's final reply in its channel as the Vision's message (a failure, not
    /// a stop, as a notice). Returns whether the run finished.
    pub(super) fn visions_post_reply(&self, owner: &Owner, terminal: &ThreadEvent) -> bool {
        let entry = match terminal {
            ThreadEvent::Finished { result, .. } | ThreadEvent::Incomplete { result, .. } => {
                let text = bounded(result.trim());
                (!text.is_empty()).then(|| ChannelEntry::Vision {
                    vision_id: owner.vision_id.clone(),
                    text,
                })
            }
            ThreadEvent::Failed { error }
                if !error.starts_with(themis_core::runtime::STOPPED_BY_YOU) =>
            {
                Some(ChannelEntry::Notice {
                    kind: NoticeKind::ChatFailed,
                    vision_id: Some(owner.vision_id.clone()),
                    thread_id: Some(owner.thread_id.clone()),
                })
            }
            _ => None,
        };
        let posted = entry.map(|entry| self.channel_append(&owner.channel_id, entry).is_ok());
        self.visions_note(
            "reply_posted",
            json!({"vision_id": owner.vision_id, "channel_id": owner.channel_id, "thread_id": owner.thread_id, "posted": posted}),
        );
        matches!(
            terminal,
            ThreadEvent::Finished { .. } | ThreadEvent::Incomplete { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use themis_core::configuration::{ApprovalMode, VisionsConfig};
    use themis_core::visions::Autonomy;

    use super::*;

    fn wren() -> Vision {
        Vision {
            id: "v1".into(),
            name: "Wren".into(),
            role: "Keeps the docs right".into(),
            shape: "cloud".into(),
            color: "teal".into(),
            model: None,
            autonomy: None,
            custom: None,
            daily_budget_usd: None,
            soul: "Careful and brief.".into(),
            paused: false,
            revision: String::new(),
        }
    }

    fn hq() -> Channel {
        Channel {
            id: "c1".into(),
            name: "hq".into(),
            topic: String::new(),
            project_root: PathBuf::from("/work/app"),
            approval_mode: ApprovalMode::Custom,
            members: vec!["v1".into()],
            revision: String::new(),
        }
    }

    /// The head the run's system message opens with (printed so the request path can be read):
    /// the Vision, its channel, the `themis` calls with this channel's ids, the rules and one
    /// table row per decision as the level decides it.
    #[test]
    fn vision_system_head_contains_the_autonomy_table() {
        let head = vision_head(&wren(), &hq(), &VisionsConfig::default());
        println!("{head}");
        assert!(head.starts_with("You are Wren, a Vision"));
        assert!(head.contains("Your role: Keeps the docs right"));
        assert!(head.contains("Careful and brief."));
        assert!(head.contains("#hq of the project at /work/app"));
        assert!(head.contains(r#"'{"projectRoot": "/work/app", "provider": "go"}'"#));
        assert!(head.contains(r#"list_channel_threads '{"channelId": "c1"}'"#));
        assert!(head.contains("Never repeat a chat's own summary as proof"));
        assert!(head.contains("Never restart, resend or replace a chat the person stopped"));
        assert!(head.ends_with(VISIONS_READ_ONLY_NOTE));
        // The note says what it really has: the read tools, and a shell for `themis` only.
        for words in ["list_dir", "search_file", "read_file", "runs only `themis`"] {
            assert!(VISIONS_READ_ONLY_NOTE.contains(words), "{words}");
        }
        // Small calls by default: wording and tests are the Vision's, the rest the person's.
        let rows: Vec<&str> = head
            .lines()
            .filter(|line| line.starts_with("- `"))
            .collect();
        assert_eq!(rows.len(), Decision::ALL.len());
        for (row, decision) in rows.iter().zip(Decision::ALL) {
            let mine = matches!(decision, Decision::Wording | Decision::Tests);
            let who = if mine { "you decide" } else { "ask the person" };
            assert_eq!(*row, format!("- `{}`: {who}", decision.id()));
        }
        // A profile may only lower the resolved level: `nearly_all` over the default `small`
        // still decides only wording and tests, and its `ask` lowers it to nothing.
        let raised = Vision {
            autonomy: Some(Autonomy::NearlyAll),
            ..wren()
        };
        let head = vision_head(&raised, &hq(), &VisionsConfig::default());
        assert!(head.contains("- `wording`: you decide"));
        assert!(head.contains("- `delete`: ask the person"));
        let lowered = Vision {
            autonomy: Some(Autonomy::Ask),
            ..wren()
        };
        let head = vision_head(&lowered, &hq(), &VisionsConfig::default());
        assert!(head.contains("- `wording`: ask the person"));
        // Custom rows can only take away; `spend` stays the person's.
        let custom = Vision {
            custom: Some(BTreeMap::from([
                ("tests".into(), false),
                ("delete".into(), true),
                ("spend".into(), true),
            ])),
            ..wren()
        };
        let head = vision_head(&custom, &hq(), &VisionsConfig::default());
        assert!(head.contains("- `wording`: you decide"));
        assert!(head.contains("- `tests`: ask the person"));
        assert!(head.contains("- `delete`: ask the person"));
        assert!(head.contains("- `spend`: ask the person"));
    }

    /// A failed send whose run never started (refused before saving, or saved but its start
    /// never recorded) ends the wake here; one that failed to spawn ends at the run's end step.
    #[test]
    fn a_save_without_a_run_releases_the_wake() {
        assert!(never_ran(ServerSend::Pending));
        assert!(never_ran(ServerSend::Saved));
        assert!(!never_ran(ServerSend::SpawnFailed));
    }

    /// When the envelope cannot be built after all (a race past the start check), every command
    /// is refused with the Vision's own reason.
    #[test]
    fn the_race_fallback_refuses_with_the_visions_text() {
        let dir = tempfile::tempdir().unwrap();
        let vision = VisionRun {
            head: String::new(),
            scope: VisionScope {
                vision_id: "v1".into(),
                channel_id: "c1".into(),
                project_root: dir.path().to_path_buf(),
            },
            port: 47123,
        };
        let plan = Arc::new(SandboxPlan::disabled(dir.path()));
        let confined = confine(Some(&vision), plan, dir.path());
        let refusal = confined.refusal().unwrap_or_default();
        assert!(
            refusal.contains("no sandbox here can hold a Vision's run"),
            "{refusal}"
        );
    }

    /// A Vision run reads, lists and searches the project with the read tools on every OS, and
    /// has the shell for `themis`; nothing else, whatever the run's tools were.
    #[test]
    fn a_vision_run_gets_its_own_read_tools_on_every_os() {
        let dir = tempfile::tempdir().unwrap();
        let approvals: Arc<dyn ApprovalHook> = Arc::new(themis_core::tools::AllowAllHook);
        let sandbox = Arc::new(SandboxPlan::disabled(dir.path()));
        let all = themis_core::tools::boxed_tools_for_run(
            dir.path(),
            Arc::clone(&approvals),
            themis_core::tools::ToolOptions::default(),
        )
        .unwrap();
        let vision = VisionRun {
            head: String::new(),
            scope: VisionScope {
                vision_id: "v1".into(),
                channel_id: "c1".into(),
                project_root: dir.path().to_path_buf(),
            },
            port: 47123,
        };
        let tools = run_tools(Some(&vision), all, dir.path(), &approvals, &sandbox);
        let mut names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
        names.sort_unstable();
        assert_eq!(names, ["list_dir", "read_file", "search_file", "shell"]);
    }

    /// Only a send refused because the chat or the app is busy is retried; one the run's own
    /// setup refused (the Vision paused or gone, Visions off) waits for the next wake.
    #[test]
    fn only_a_busy_send_is_retried() {
        let busy = format!(
            "thread 't' {}: a run is already in progress",
            crate::state::threads::BUSY
        );
        assert_eq!(start_refusal(&busy, ServerSend::Pending), Some("busy"));
        assert_eq!(
            start_refusal("This Vision is paused", ServerSend::Pending),
            Some("not_started")
        );
        assert_eq!(start_refusal("anything", ServerSend::Saved), Some("busy"));
        assert_eq!(start_refusal("spawn failed", ServerSend::SpawnFailed), None);
    }

    #[test]
    fn a_long_reply_is_cut_on_a_character() {
        let long = "é".repeat(MAX_REPLY_CHARS + 5);
        let cut = bounded(&long);
        assert_eq!(cut.chars().count(), MAX_REPLY_CHARS + 1);
        assert!(cut.ends_with('…'));
        assert_eq!(bounded("short"), "short");
    }
}
