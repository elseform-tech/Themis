//! Routines through the real app server: a chief adds one with its credential, the channel says
//! so, and the person lists, pauses and removes it.

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_chiefs_routine_is_listed_paused_and_removed_by_the_person() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let (juno, release) = fx.chief_and_channel("Juno", "release");
    let (_, chief) = fx.vision_run(&juno, &release).await;
    let routine = json!({"schedule": {"days": [1, 2, 3, 4, 5], "time": "09:00"}, "instruction": "Summarise yesterday"});
    let saved = fx.agent_call_ok(&chief, "save_routine", json!({"routine": routine}));
    let id = saved["id"].as_str().unwrap().to_owned();
    assert!(fx
        .entries(&release)
        .iter()
        .any(|e| e["type"] == "routine_set" && e["routine_id"] == id.as_str()));
    let listed = fx.call_ok("list_routines", json!({"channelId": release}));
    assert_eq!(listed[0]["instruction"], "Summarise yesterday");
    fx.call_ok(
        "set_routine_enabled",
        json!({"routineId": id, "enabled": false}),
    );
    assert_eq!(
        fx.call_ok("list_routines", json!({"channelId": release}))[0]["enabled"],
        false
    );
    fx.call_ok("delete_routine", json!({"routineId": id}));
    assert_eq!(
        fx.call_ok("list_routines", json!({"channelId": release})),
        json!([])
    );
    // A worker keeps no routines.
    let (rook, work) = fx.vision_and_channel("Rook", "work");
    let (_, worker) = fx.vision_run(&rook, &work).await;
    assert!(!fx
        .agent_call(&worker, "save_routine", json!({"routine": routine}))
        .status
        .success());
    fx.shut_down().await;
}
