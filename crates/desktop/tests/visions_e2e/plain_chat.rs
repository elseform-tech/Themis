//! A Vision's work chats in plain chat, through the real app server: listed with their Vision
//! (its own chats hidden, the channel lists them too), a chat a Vision starts is announced to
//! plain chat with its Vision, the person typing in it from plain chat takes it
//! over exactly as from the channel, and a released chat is announced again without one.

use super::*;

/// The next `thread-changed` event for `thread`, within five seconds.
async fn thread_changed(events: &mut tokio::net::TcpStream, thread: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = Client::next_event(events).await.unwrap();
            if event["name"] == "thread-changed" && event["payload"]["thread_id"] == thread {
                break event["payload"].clone();
            }
        }
    })
    .await
    .expect("thread-changed announced")
}

#[tokio::test(flavor = "multi_thread")]
async fn owned_chats_are_listed_with_their_vision() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let (vision, channel) = fx.vision_and_channel("Wren", "hq");
    let plain = fx.thread().await;
    let work = fx.thread().await;
    let own_chat = fx.thread().await;
    fx.own(&work, &vision, &channel, OwnedKind::Work);
    fx.own(&own_chat, &vision, &channel, OwnedKind::VisionChat);

    // A work chat shows in plain chat with its Vision; the Vision's own chat stays hidden.
    let listed = fx.call_ok("list_threads", json!({"projectRoot": fx.project}));
    let by_id = |id: &str| {
        listed
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == id)
            .cloned()
    };
    assert!(by_id(&own_chat).is_none(), "{listed}");
    assert_eq!(by_id(&plain).unwrap()["vision"], Value::Null);
    let shown = json!({"id": vision, "name": "Wren", "shape": "cloud", "color": "teal"});
    assert_eq!(by_id(&work).unwrap()["vision"], shown);
    assert_eq!(
        fx.call_ok("get_thread", json!({"threadId": work}))["vision"],
        shown
    );
    let in_channel = fx.call_ok("list_channel_threads", json!({"channelId": channel}));
    let in_channel = in_channel.as_array().unwrap();
    assert_eq!(in_channel.len(), 1, "{in_channel:?}");
    assert_eq!(in_channel[0]["id"], work.as_str());
    assert_eq!(in_channel[0]["owner"]["vision_id"], vision.as_str());
    assert_eq!(in_channel[0]["owner"]["hold"], "none");

    // Discarding an owned chat forgets its owner.
    fx.call_ok("discard_thread", json!({"threadId": work}));
    assert!(fx.state.owner_of(&work).is_none());
    let in_channel = fx.call_ok("list_channel_threads", json!({"channelId": channel}));
    assert_eq!(in_channel, json!([]));
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn typing_in_plain_chat_takes_an_owned_chat_over() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let provider = quick_provider().await;
    fx.use_provider(&provider).await;
    let (vision, channel) = fx.vision_and_channel("Wren", "hq");
    let work = fx.thread().await;
    fx.own(&work, &vision, &channel, OwnedKind::Work);
    assert!(fx.listed_ids().contains(&work));

    // What the app's composer sends from plain chat.
    fx.call_ok(
        "send_message",
        json!({"threadId": work, "text": "let me do this bit", "reasoningEffort": null,
            "attachments": []}),
    );
    assert_eq!(owner_hold(&fx.state, &work), Some(Hold::TakenOver));
    let in_channel = fx.call_ok("list_channel_threads", json!({"channelId": channel}));
    assert_eq!(in_channel[0]["owner"]["hold"], "taken_over");
    wait_until(|| async { !fx.running(&work).await }).await;
    // Still marked in plain chat while the person holds it.
    let shown = fx.call_ok("get_thread", json!({"threadId": work}));
    assert_eq!(shown["vision"]["name"], "Wren");

    // Hand back from plain chat: the same call the channel makes.
    fx.call_ok("hand_back_thread", json!({"threadId": work}));
    assert_eq!(owner_hold(&fx.state, &work), Some(Hold::None));
    assert_eq!(fx.entries(&channel).last().unwrap()["kind"], "handed_back");
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_visions_new_chat_and_its_release_are_announced_to_plain_chat() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let provider = quick_provider().await;
    fx.use_provider(&provider).await;
    let (vision, channel) = fx.vision_and_channel("Wren", "hq");
    let (_own_chat, token) = fx.vision_run(&vision, &channel).await;
    let mut events = Client::new(fx.data.clone()).subscribe().await.unwrap();

    let work = fx.vision_starts_chat(&token);
    let created = thread_changed(&mut events, &work).await;
    assert_eq!(created["kind"], "created");
    assert_eq!(created["thread"]["vision"]["id"], vision.as_str());
    assert_eq!(created["thread"]["vision"]["color"], "teal");

    fx.call_ok("delete_vision", json!({"visionId": vision}));
    let released = thread_changed(&mut events, &work).await;
    assert_eq!(released["kind"], "updated");
    assert!(released["thread"]["vision"].is_null(), "{released}");
    fx.shut_down().await;
}

/// The next event named `name`, within five seconds.
async fn next_named(events: &mut tokio::net::TcpStream, name: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = Client::next_event(events).await.unwrap();
            if event["name"] == name {
                break event["payload"].clone();
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{name} announced"))
}

#[tokio::test(flavor = "multi_thread")]
async fn queueing_in_an_owned_chat_announces_its_channel() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let provider = stalled_provider().await;
    fx.use_provider(&provider).await;
    let (vision, channel) = fx.vision_and_channel("Wren", "hq");
    let work = fx.thread().await;
    fx.start_stalled_run(&work, &provider).await;
    fx.own(&work, &vision, &channel, OwnedKind::Work);
    let mut events = Client::new(fx.data.clone()).subscribe().await.unwrap();

    fx.call_ok(
        "enqueue_message",
        json!({"threadId": work, "text": "then tidy up", "reasoningEffort": null}),
    );
    let changed = next_named(&mut events, "visions-changed").await;
    assert_eq!(changed["channel_id"], channel.as_str());
    assert_eq!(changed["vision_id"], vision.as_str());
    fx.state.stop_thread(work.clone()).await.unwrap();
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_channel_announces_its_released_chats_to_plain_chat() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    fx.no_wakes();
    let (vision, channel) = fx.vision_and_channel("Wren", "hq");
    let work = fx.thread().await;
    fx.own(&work, &vision, &channel, OwnedKind::Work);
    let mut events = Client::new(fx.data.clone()).subscribe().await.unwrap();

    fx.call_ok("delete_channel", json!({"channelId": channel}));
    let released = thread_changed(&mut events, &work).await;
    assert_eq!(released["kind"], "updated");
    assert!(released["thread"]["vision"].is_null(), "{released}");
    fx.shut_down().await;
}

/// A Vision's own chat stays out of plain chat: renaming it announces nothing, while the same
/// call on a plain chat is announced.
#[tokio::test(flavor = "multi_thread")]
async fn a_visions_own_chat_is_never_announced_to_plain_chat() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    fx.no_wakes();
    let (vision, channel) = fx.vision_and_channel("Wren", "hq");
    let own_chat = fx.thread().await;
    let plain = fx.thread().await;
    fx.own(&own_chat, &vision, &channel, OwnedKind::VisionChat);
    let mut events = Client::new(fx.data.clone()).subscribe().await.unwrap();

    fx.call_ok(
        "rename_thread",
        json!({"threadId": own_chat, "title": "Mine"}),
    );
    fx.call_ok(
        "rename_thread",
        json!({"threadId": plain, "title": "Yours"}),
    );
    let first = next_named(&mut events, "thread-changed").await;
    assert_eq!(first["thread_id"], plain.as_str(), "{first}");
    fx.shut_down().await;
}
