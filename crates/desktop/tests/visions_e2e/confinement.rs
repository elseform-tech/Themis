//! Whether this computer can confine a Vision's run, as Settings › Visions reads it through
//! the real app server: `can_run` with the settings, false when no sandbox here can hold a
//! Vision in its envelope (every wake would be refused).

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn the_visions_settings_say_whether_visions_can_run_here() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let read = fx.call_ok("get_visions_settings", json!({}));
    // Seatbelt always can on macOS; Linux needs Landlock ABI 4 (6.7), which a runner may lack.
    if cfg!(target_os = "macos") {
        assert_eq!(read["can_run"], true, "{read}");
    }
    assert!(read["can_run"].is_boolean(), "{read}");

    fx.state.set_sandbox_backends_unavailable(true);
    let read = fx.call_ok("get_visions_settings", json!({}));
    assert_eq!(read["can_run"], false, "{read}");
    // A write answers with it too, so the screen keeps it.
    let revision = read["revision"].clone();
    let written = fx.call_ok(
        "update_visions_settings",
        json!({"changes": {"max_wakes": 5}, "revision": revision}),
    );
    assert_eq!(written["can_run"], false, "{written}");
    assert_eq!(written["settings"]["max_wakes"], 5, "{written}");
    fx.shut_down().await;
}

/// The person's hooks and command-based MCP servers never start for a Vision's run (its
/// envelope has no plugins); they still do for an ordinary chat.
#[tokio::test(flavor = "multi_thread")]
async fn hooks_and_command_mcp_servers_do_not_start_for_a_vision_run() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let marks = fx.data.join("marks");
    std::fs::create_dir_all(&marks).unwrap();
    let hook_mark = marks.join("hook");
    let mcp_mark = marks.join("mcp");
    fx.state
        .plugin_action(json!({"action": "save", "scope": "global", "spec": {
            "name": "personal",
            "hooks": [{"name": "mark", "event": "PromptSubmit", "enabled": true,
                "command": format!("echo ran >> '{}'", hook_mark.display())}],
            "mcp": {"marker": {"command": "sh", "enabled": true,
                "args": ["-c", format!("echo ran >> '{}'", mcp_mark.display())]}},
            "files": {}
        }}))
        .await
        .unwrap();
    let provider = quick_provider().await;
    fx.use_provider(&provider).await;
    let (vision, channel) = fx.chief_and_channel_in("Wren", "work", "yolo");
    fx.call_ok(
        "post_channel_message",
        json!({"channelId": channel, "text": "@Wren look around"}),
    );
    if first_vision_turn(&fx, &channel).await {
        fx.shut_down().await;
        return;
    }
    let own = fx
        .state
        .owners_where(|owner| owner.vision_id == vision && owner.kind == OwnedKind::VisionChat);
    wait_until(|| async { !fx.running(&own[0].thread_id).await }).await;
    assert!(!hook_mark.exists(), "no hook ran for the Vision's run");
    assert!(
        !mcp_mark.exists(),
        "no MCP server started for the Vision's run"
    );

    // The same hook runs in an ordinary chat, so the check above is a real one.
    let plain = fx.thread().await;
    fx.call_ok(
        "set_thread_approval_mode",
        json!({"threadId": plain, "mode": "yolo"}),
    );
    fx.call_ok(
        "send_message",
        json!({"threadId": plain, "text": "hello", "reasoningEffort": null}),
    );
    wait_until(|| async { hook_mark.exists() }).await;
    wait_until(|| async { !fx.running(&plain).await }).await;
    fx.shut_down().await;
}
