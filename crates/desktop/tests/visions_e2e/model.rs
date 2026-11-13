//! A Vision's model through the real app server: saving a new model moves the Vision's own
//! chats to it, while its work chats and a model the person picked in its chat stay as they are.

use super::*;

fn model_of(fx: &Fixture, thread: &str) -> String {
    let info = fx.call_ok("get_thread", json!({"threadId": thread}));
    info["model"].as_str().unwrap_or_default().to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_vision_model_reaches_its_own_chats_only() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    fx.no_wakes();
    let (vision, channel) = fx.vision_and_channel("Wren", "hq");
    let own_chat = fx.thread().await;
    let work = fx.thread().await;
    fx.own(&own_chat, &vision, &channel, OwnedKind::VisionChat);
    fx.own(&work, &vision, &channel, OwnedKind::Work);

    let mut profile = fx
        .call_ok("list_visions", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == vision.as_str())
        .unwrap()
        .clone();
    let revision = profile["revision"].clone();
    profile["model"] = json!("other-model");
    let saved = fx.call_ok(
        "save_vision",
        json!({"vision": profile, "revision": revision}),
    );
    assert_eq!(model_of(&fx, &own_chat), "other-model");
    assert_eq!(
        model_of(&fx, &work),
        "test-model",
        "work chats keep their model"
    );

    fx.call_ok(
        "set_provider",
        json!({"threadId": own_chat, "provider": "go", "model": "picked-by-hand"}),
    );
    let mut renamed = saved.clone();
    renamed["name"] = json!("Wren B");
    fx.call_ok(
        "save_vision",
        json!({"vision": renamed, "revision": saved["revision"]}),
    );
    assert_eq!(model_of(&fx, &own_chat), "picked-by-hand");
    fx.shut_down().await;
}
