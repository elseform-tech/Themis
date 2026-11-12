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
                name: "hq".into(),
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
        fx.state
            .visions_send(sink(), &info.id, "go".into(), None, Vec::new())
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
