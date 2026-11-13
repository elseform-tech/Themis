//! Whose words a message in a Vision's work chat is, through the real app server: a Vision's
//! message (sent, or queued and sent from the queue) is saved as the Vision's and never counts
//! as what the person said, so no memory pass reads it; the person's own message still does.

use super::*;

/// The memory pass's system message starts so.
const PASS: &str = "You keep an assistant's memory";

/// The `memory`/`event` lines of the app's diagnostics.
fn memory_logs(data: &std::path::Path, event: &str) -> Vec<Value> {
    let mut text = String::new();
    if let Ok(entries) = std::fs::read_dir(data.join("logs")) {
        for entry in entries.flatten() {
            text.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
        }
    }
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["service"] == "memory" && line["event"] == event)
        .map(|line| line["details"].clone())
        .collect()
}

/// The chat's user messages as `get_thread_history` returns them: text and Vision.
fn user_messages(fx: &Fixture, thread: &str) -> Vec<(String, Value)> {
    let history = fx.call_ok("get_thread_history", json!({"threadId": thread}));
    history
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["kind"] == "user")
        .map(|item| {
            let text = item["text"].as_str().unwrap().to_owned();
            (text, item.get("vision_id").cloned().unwrap_or(Value::Null))
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_visions_message_is_not_the_persons_words_but_the_persons_own_is() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let provider = quick_provider().await;
    fx.use_provider(&provider).await;
    let (vision, channel) = fx.vision_and_channel("Wren", "work");
    let (_chat, token) = fx.vision_run(&vision, &channel).await;
    // Visions don't wake on their own here; the memory models are the scripted provider's.
    let config = json!({"visions": {"enabled": false}, "memory": {"models": ["test-model", "mem-two", "mem-three"]}});
    std::fs::write(fx.data.join("runtime.jsonc"), config.to_string()).unwrap();
    fx.state.set_memory_jobs_for_test(true, 600_000, 600_000);
    let work = fx.vision_starts_chat(&token);

    // The Vision sends, then queues while its run goes; both are its own words.
    let sent = "From now on we always install packages with pnpm, never npm.";
    let send = json!({"threadId": work, "text": sent});
    let run = json_of(&fx.agent_call(&token, "send_message", send))["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let queued = "Our deploys go out on Tuesdays and never on a Friday afternoon.";
    let queue = json!({"threadId": work, "text": queued});
    json_of(&fx.agent_call(&token, "enqueue_message", queue));
    wait_until(|| async { user_messages(&fx, &work).len() == 2 && !fx.running(&work).await }).await;
    let vision_id = json!(vision);
    assert_eq!(
        user_messages(&fx, &work),
        [
            (sent.to_owned(), vision_id.clone()),
            (queued.to_owned(), vision_id.clone())
        ]
    );
    // Each of its turns' memory passes found nothing the person said.
    wait_until(|| async {
        let skipped = memory_logs(&fx.data, "pass_skipped");
        skipped.len() >= 2
            && skipped.iter().any(|line| line["run_id"] == run.as_str())
            && skipped
                .iter()
                .all(|line| line["reason"] == "no_person_message")
    })
    .await;

    // The person's own message in the same chat is theirs: saved without a Vision, and its
    // turn's memory pass reads it.
    let own = "Remember that the staging database is reset every Monday morning.";
    fx.call_ok(
        "send_message",
        json!({"threadId": work, "text": own, "reasoningEffort": null}),
    );
    wait_until(|| async {
        provider
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|request| {
                let body = String::from_utf8_lossy(&request.body);
                body.contains(PASS) && body.contains(own)
            })
    })
    .await;
    assert_eq!(user_messages(&fx, &work)[2], (own.to_owned(), Value::Null));
    let passes: Vec<String> = provider
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .filter(|body| body.contains(PASS))
        .collect();
    assert!(
        passes
            .iter()
            .all(|body| !body.contains(sent) && !body.contains(queued)),
        "{passes:?}"
    );
    fx.shut_down().await;
}
