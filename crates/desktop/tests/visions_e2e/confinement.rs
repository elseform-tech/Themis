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
