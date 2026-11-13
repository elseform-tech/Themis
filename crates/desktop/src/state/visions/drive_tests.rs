use std::path::Path;
use std::sync::Arc;

use themis_core::agent_cli::VisionScope;
use themis_core::configuration::ApprovalMode;

use super::*;
use crate::sink::{ChannelSink, EventSink};
use crate::state::AppState;

struct Setup {
    _dir: tempfile::TempDir,
    state: AppState,
    scope: VisionScope,
    vision: Vision,
}

fn setup(model: Option<&str>) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let project = project.canonicalize().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let vision = state
        .vision_save(
            Vision {
                id: String::new(),
                name: "Wren".into(),
                role: String::new(),
                shape: "cloud".into(),
                color: "teal".into(),
                model: model.map(str::to_owned),
                autonomy: None,
                custom: None,
                daily_budget_usd: None,
                soul: String::new(),
                paused: false,
                revision: String::new(),
            },
            None,
        )
        .unwrap();
    let channel = state
        .channel_save(
            Channel {
                id: String::new(),
                name: "work".into(),
                topic: String::new(),
                project_root: project.clone(),
                approval_mode: ApprovalMode::Custom,
                members: vec![vision.id.clone()],
                revision: String::new(),
            },
            None,
        )
        .unwrap();
    let scope = VisionScope {
        vision_id: vision.id.clone(),
        channel_id: channel.id,
        project_root: project,
        decides: Vec::new(),
    };
    Setup {
        _dir: dir,
        state,
        scope,
        vision,
    }
}

fn sink() -> Arc<dyn EventSink> {
    Arc::new(ChannelSink::channel().0)
}

/// A Vision's work chat runs on the Vision's model, or the app's default; never one the call
/// names (the server takes neither provider nor model from a Vision).
#[tokio::test]
async fn a_work_chat_runs_on_the_visions_model_or_the_default() {
    let with = setup(Some("vision-model"));
    let info = with
        .state
        .visions_start_work_chat(&with.scope)
        .await
        .unwrap();
    assert_eq!(info.model, "vision-model");
    assert_eq!(info.approval_mode, ApprovalMode::Custom);

    let without = setup(None);
    let info = without
        .state
        .visions_start_work_chat(&without.scope)
        .await
        .unwrap();
    let default = without.state.inner.settings.get().await.default_model;
    let plain = without
        .state
        .create_thread(
            Path::new(&without.scope.project_root)
                .to_string_lossy()
                .into_owned(),
            crate::types::ProviderKind::Go,
            Some(default),
        )
        .await
        .unwrap();
    assert_eq!(info.model, plain.model, "the app's default model");
}

/// The send checks the hold and the pause again right before it goes, so a Stop, take over
/// or pause that lands after the server's first look still holds the Vision back.
#[tokio::test]
async fn a_send_rechecks_the_hold_and_the_pause() {
    let fx = setup(None);
    let info = fx.state.visions_start_work_chat(&fx.scope).await.unwrap();
    let send = || {
        fx.state.visions_send(
            sink(),
            &fx.scope.vision_id,
            &info.id,
            "go".into(),
            None,
            Vec::new(),
        )
    };
    for hold in [Hold::StoppedByPerson, Hold::TakenOver] {
        fx.state.owner_hold(&info.id, |_| hold).unwrap();
        let refused = send().await.unwrap_err();
        assert_eq!(refused, themis_core::prompts::AGENT_CLI_HELD_BY_PERSON);
    }
    fx.state.owner_hold(&info.id, |_| Hold::None).unwrap();
    let revision = fx.vision.revision.clone();
    fx.state
        .vision_save(
            Vision {
                paused: true,
                ..fx.vision.clone()
            },
            Some(&revision),
        )
        .unwrap();
    let refused = send().await.unwrap_err();
    assert_eq!(refused, themis_core::prompts::AGENT_CLI_VISION_PAUSED);
    // Nothing started.
    assert!(!fx.state.get_thread(&info.id).await.unwrap().running);
    // A chat it does not own is never written in.
    fx.state.owner_release(&info.id).unwrap();
    assert!(send().await.is_err());
}

/// The chat's user messages, oldest first, each with the Vision that sent it.
fn user_messages(fx: &Setup, thread_id: &str) -> Vec<(String, Option<String>)> {
    let page = fx.state.inner.transcript.history_after(thread_id, 0, 100);
    page.unwrap()
        .into_iter()
        .filter_map(|(_, item)| match item {
            crate::transcript::HistoryItem::User {
                text, vision_id, ..
            } => Some((text, vision_id)),
            _ => None,
        })
        .collect()
}

async fn until_idle(fx: &Setup, thread_id: &str) {
    for _ in 0..500 {
        if !fx.state.get_thread(thread_id).await.unwrap().running {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the chat still runs");
}

/// A Vision's message in a goal's chat, sent or queued, is saved as the Vision's and joins the
/// goal as no "Also asked" line: it is not the person asking (so the agent may not pause the
/// goal for it either). The person's own message still joins it.
#[tokio::test]
async fn a_visions_message_is_not_the_persons_for_the_goal() {
    let fx = setup(Some("vision-model"));
    // Priced, so the Vision's budget can be checked.
    fx.state
        .set_model_catalog_for_test(vec![themis_core::providers::GoModel {
            id: "vision-model".into(),
            price: Some(themis_core::providers::ModelPrice::default()),
            ..Default::default()
        }])
        .await;
    let work = fx
        .state
        .visions_start_work_chat(&fx.scope)
        .await
        .unwrap()
        .id;
    let vision = fx.scope.vision_id.clone();
    fx.state
        .begin_goal(sink().as_ref(), &work, "ship the release");
    // No provider key here: the run fails to start, after the message was saved and the goal
    // heard it.
    let sent = "also add a changelog".to_owned();
    let _ = fx
        .state
        .visions_send(sink(), &vision, &work, sent.clone(), None, Vec::new())
        .await;
    until_idle(&fx, &work).await;
    let queued = crate::types::QueuedMessage::new(
        "and a release note".into(),
        Vec::new(),
        None,
        Some(vision.clone()),
    );
    fx.state
        .enqueue(sink(), work.clone(), queued)
        .await
        .unwrap();
    for _ in 0..500 {
        if user_messages(&fx, &work).len() == 2
            && !fx.state.get_thread(&work).await.unwrap().running
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let goal = fx.state.goal_for(&work).unwrap();
    assert!(goal.also_asked.is_empty(), "{:?}", goal.also_asked);
    assert!(!fx.state.person_spoke_since_goal_turn(&work));
    assert_eq!(
        user_messages(&fx, &work),
        [
            (sent, Some(vision.clone())),
            ("and a release note".to_owned(), Some(vision.clone())),
        ]
    );

    let _ = fx
        .state
        .send_message(sink(), work.clone(), "also bump the version".into())
        .await;
    let goal = fx.state.goal_for(&work).unwrap();
    assert_eq!(goal.also_asked, ["also bump the version"]);
    assert!(fx.state.person_spoke_since_goal_turn(&work));
    assert_eq!(user_messages(&fx, &work)[2].1, None);
}

/// A chat recorded as the Vision's own chat in `scope`'s channel.
async fn own_chat(fx: &Setup) -> String {
    let info = fx
        .state
        .create_thread(
            fx.scope.project_root.to_string_lossy().into_owned(),
            crate::types::ProviderKind::Go,
            Some("vision-model".to_owned()),
        )
        .await
        .unwrap();
    fx.state
        .owner_set(Owner {
            thread_id: info.id.clone(),
            vision_id: fx.vision.id.clone(),
            channel_id: fx.scope.channel_id.clone(),
            kind: OwnedKind::VisionChat,
            hold: Hold::None,
        })
        .unwrap();
    info.id
}

async fn model_of(fx: &Setup, thread_id: &str) -> String {
    fx.state.get_thread(thread_id).await.unwrap().model
}

/// Changing a Vision's model moves its own chats to the new one from their next turn; the
/// work chats it started keep theirs, and saving without a model change leaves a model the
/// person picked by hand alone.
#[tokio::test]
async fn a_vision_chat_follows_its_profile_model() {
    let fx = setup(Some("vision-model"));
    let own = own_chat(&fx).await;
    let work = fx
        .state
        .visions_start_work_chat(&fx.scope)
        .await
        .unwrap()
        .id;

    let current = fx.state.vision_get(&fx.vision.id).unwrap();
    let revision = current.revision.clone();
    let moved = Vision {
        model: Some("other-model".into()),
        ..current
    };
    let saved = fx.state.visions_save(moved, Some(&revision)).await.unwrap();
    assert_eq!(model_of(&fx, &own).await, "other-model");
    assert_eq!(
        model_of(&fx, &work).await,
        "vision-model",
        "work chats keep theirs"
    );

    fx.state
        .set_provider(
            own.clone(),
            crate::types::ProviderKind::Go,
            Some("picked-by-hand".into()),
        )
        .await
        .unwrap();
    let renamed = Vision {
        name: "Wren B".into(),
        ..saved.clone()
    };
    fx.state
        .visions_save(renamed, Some(&saved.revision))
        .await
        .unwrap();
    assert_eq!(model_of(&fx, &own).await, "picked-by-hand");
}

/// The person's Stop holds the chat even when the hold cannot be saved: it is kept while the
/// app runs, so the Vision's writes there are still refused, and the failed save is logged.
#[cfg(unix)]
#[tokio::test]
async fn a_stop_holds_the_chat_even_when_the_hold_cannot_be_saved() {
    use std::os::unix::fs::PermissionsExt;
    let fx = setup(None);
    let work = fx
        .state
        .visions_start_work_chat(&fx.scope)
        .await
        .unwrap()
        .id;
    let dir = fx.state.inner.visions_dir.clone();
    let mode = |bits| std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(bits));
    mode(0o500).unwrap();
    fx.state.visions_person_stopped(&work);
    mode(0o700).unwrap();
    assert_eq!(
        fx.state.owner_of(&work).map(|owner| owner.hold),
        Some(Hold::StoppedByPerson)
    );
    let refused = fx
        .state
        .visions_send(
            sink(),
            &fx.scope.vision_id,
            &work,
            "go on".into(),
            None,
            Vec::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(refused, themis_core::prompts::AGENT_CLI_HELD_BY_PERSON);
    let mut logged = String::new();
    for entry in std::fs::read_dir(fx._dir.path().join("logs"))
        .unwrap()
        .flatten()
    {
        logged.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
    }
    assert!(logged.contains("\"saved\":false"), "{logged}");
}

/// The person's post is recorded and wakes the channel's Vision even when a stopped chat's
/// hold cannot be released (the failure is logged): one outcome, never a post nobody hears.
#[tokio::test]
async fn a_post_wakes_even_when_a_hold_cannot_be_released() {
    let fx = setup(None);
    let work = fx
        .state
        .visions_start_work_chat(&fx.scope)
        .await
        .unwrap()
        .id;
    fx.state
        .owner_hold(&work, |_| Hold::StoppedByPerson)
        .unwrap();
    // Edited on disk since the store read it: the release is refused.
    let owners = fx.state.inner.visions_dir.join("owners.json");
    let text = std::fs::read_to_string(&owners).unwrap();
    std::fs::write(&owners, format!("{text}\n")).unwrap();
    let posted = fx
        .state
        .visions_post(&sink(), &fx.scope.channel_id, "@Wren carry on");
    assert!(posted.is_ok(), "{posted:?}");
    let mut logged = String::new();
    for entry in std::fs::read_dir(fx._dir.path().join("logs"))
        .unwrap()
        .flatten()
    {
        logged.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
    }
    let events: Vec<serde_json::Value> = logged
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|line: &serde_json::Value| line["service"] == "visions")
        .collect();
    let woke = events
        .iter()
        .any(|line| line["event"] == "wake" && line["details"]["reason"] == "mentioned");
    assert!(woke, "{logged}");
    let note = events
        .iter()
        .find(|line| line["event"] == "channel_posted")
        .unwrap();
    assert_eq!(note["details"]["release_failed"], 1, "{note}");
}

/// When a Vision (or its channel) goes, its work chats become ordinary chats: the messages it
/// queued there, which nothing would ever send, are dropped; the person's own stay.
#[tokio::test]
async fn releasing_a_chat_drops_the_visions_queued_messages() {
    use crate::types::QueuedMessage;
    for by_channel in [false, true] {
        let fx = setup(None);
        let work = fx
            .state
            .visions_start_work_chat(&fx.scope)
            .await
            .unwrap()
            .id;
        let vision = Some(fx.scope.vision_id.clone());
        fx.state
            .edit_queue(sink().as_ref(), &work, |queue| {
                queue.push(QueuedMessage::new("mine".into(), Vec::new(), None, None));
                queue.push(QueuedMessage::new("its".into(), Vec::new(), None, vision));
                Ok(())
            })
            .unwrap();
        if by_channel {
            fx.state
                .visions_delete_channel(&sink(), &fx.scope.channel_id)
                .unwrap();
        } else {
            fx.state
                .visions_delete(&sink(), &fx.scope.vision_id)
                .unwrap();
        }
        assert!(fx.state.owner_of(&work).is_none());
        let left: Vec<String> = fx
            .state
            .queue_for(&work)
            .into_iter()
            .map(|message| message.text)
            .collect();
        assert_eq!(left, ["mine"], "by channel: {by_channel}");
    }
}

/// Every open window hears a Vision's message as the Vision's: the live `user_message` event
/// carries its id (the person's own carries none), so the chat can name who wrote it.
#[tokio::test]
async fn a_visions_message_is_announced_as_the_visions() {
    use crate::sink::TestEvent;
    use crate::types::ThreadEvent;
    let fx = setup(Some("vision-model"));
    fx.state
        .set_model_catalog_for_test(vec![themis_core::providers::GoModel {
            id: "vision-model".into(),
            price: Some(themis_core::providers::ModelPrice::default()),
            ..Default::default()
        }])
        .await;
    let work = fx
        .state
        .visions_start_work_chat(&fx.scope)
        .await
        .unwrap()
        .id;
    let (sink, mut events) = ChannelSink::channel();
    let sink: Arc<dyn EventSink> = Arc::new(sink);
    let vision = fx.scope.vision_id.clone();
    let _ = fx
        .state
        .visions_send(
            Arc::clone(&sink),
            &vision,
            &work,
            "its".into(),
            None,
            Vec::new(),
        )
        .await;
    until_idle(&fx, &work).await;
    let _ = fx
        .state
        .send_message(Arc::clone(&sink), work.clone(), "mine".into())
        .await;
    let mut announced = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let TestEvent::Thread(envelope) = event {
            if let ThreadEvent::UserMessage {
                text, vision_id, ..
            } = envelope.event
            {
                announced.push((text, vision_id));
            }
        }
    }
    assert_eq!(
        announced,
        [("its".to_owned(), Some(vision)), ("mine".to_owned(), None)]
    );
}
