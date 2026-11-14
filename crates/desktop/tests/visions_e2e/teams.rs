//! Teams through the real app server: a channel names its chief, and its autonomy and budget
//! are the team's.

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn list_channels_shows_chief_autonomy_and_budget() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    fx.no_wakes();
    let (juno, _) = fx.vision_and_channel("Juno", "scratch");
    let (rook, _) = fx.vision_and_channel("Rook", "other");
    let saved = fx.call_ok(
        "save_channel",
        json!({"channel": {"name": "release", "project_root": fx.project, "approval_mode": "custom",
            "members": [juno, rook], "chief": juno, "autonomy": "ask", "daily_budget_usd": 0.4},
            "revision": null}),
    );
    let listed = fx.call_ok("list_channels", json!({"projectRoot": fx.project}));
    let release = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|channel| channel["id"] == saved["id"])
        .unwrap();
    assert_eq!(release["chief"], juno.as_str());
    assert_eq!(release["autonomy"], "ask");
    assert_eq!(release["daily_budget_usd"], 0.4);
    let administration = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|channel| channel["name"] == "administration")
        .unwrap();
    assert_eq!(administration["chief"], "admin");
    // A chief who is not a member is refused.
    let refused = fx.call(
        "save_channel",
        json!({"channel": {"name": "stray", "project_root": fx.project, "approval_mode": "custom",
            "members": [rook], "chief": juno}, "revision": null}),
    );
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("chief"));
    fx.shut_down().await;
}
