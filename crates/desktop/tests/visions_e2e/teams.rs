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

/// `# release` led by Juno with Rook working; returns (channel, juno, rook).
fn release_team(fx: &Fixture) -> (String, String, String) {
    let (juno, _) = fx.vision_and_channel("Juno", "scratch");
    let (rook, _) = fx.vision_and_channel("Rook", "other");
    let saved = fx.call_ok(
        "save_channel",
        json!({"channel": {"name": "release", "project_root": fx.project, "approval_mode": "custom",
            "members": [juno, rook], "chief": juno}, "revision": null}),
    );
    (saved["id"].as_str().unwrap().to_owned(), juno, rook)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_task_reaches_the_worker_and_its_report_reaches_the_chief() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let (channel, juno, rook) = release_team(&fx);
    let (_, chief) = fx.vision_run(&juno, &channel).await;
    let (_, worker) = fx.vision_run(&rook, &channel).await;

    let team = fx.agent_call_ok(&chief, "list_team", json!({}));
    assert_eq!(team[0]["id"], rook.as_str(), "{team}");
    let tasked = fx.agent_call_ok(
        &chief,
        "assign_task",
        json!({"workerId": rook, "task": "Fix the build"}),
    );
    assert_eq!(tasked["seq"], 1, "{tasked}");
    // The worker reads its task in its own team log, and reports.
    let log = fx.agent_call_ok(&worker, "get_team_log", json!({}));
    assert_eq!(log["entries"][0]["entry"]["kind"], "task", "{log}");
    assert_eq!(log["entries"][0]["entry"]["text"], "Fix the build", "{log}");
    fx.agent_call_ok(&worker, "report", json!({"text": "Build is green"}));
    fx.agent_call_ok(
        &worker,
        "push_back",
        json!({"text": "The fix drops a test", "safer": "Keep it and mark it slow"}),
    );
    let log = fx.agent_call_ok(&chief, "get_team_log", json!({"workerId": rook}));
    let kinds: Vec<&str> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line["entry"]["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["task", "report", "pushback"], "{log}");

    // The chief drives no chat, and a worker tasks nobody.
    assert!(!fx
        .agent_call(&chief, "create_thread", json!({"projectRoot": fx.project}))
        .status
        .success());
    assert!(!fx
        .agent_call(
            &worker,
            "assign_task",
            json!({"workerId": juno, "task": "x"})
        )
        .status
        .success());
    // A chief tasks only its own channel's workers.
    let stranger = fx.agent_call(
        &chief,
        "assign_task",
        json!({"workerId": "stranger", "task": "x"}),
    );
    assert!(!stranger.status.success());
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chief_who_no_longer_leads_tasks_nobody() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let (channel, juno, rook) = release_team(&fx);
    let (_, chief) = fx.vision_run(&juno, &channel).await;
    // Rook leads now; Juno's run started as chief.
    let listed = fx.call_ok("list_channels", json!({"projectRoot": fx.project}));
    let mut release = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == channel.as_str())
        .unwrap()
        .clone();
    let revision = release["revision"].clone();
    release["chief"] = json!(rook);
    fx.call_ok(
        "save_channel",
        json!({"channel": release, "revision": revision}),
    );
    let refused = fx.agent_call(
        &chief,
        "assign_task",
        json!({"workerId": juno, "task": "x"}),
    );
    assert!(!refused.status.success());
    fx.shut_down().await;
}

/// `release` with `chief` set to `vision`, saved over its current revision.
fn make_chief(fx: &Fixture, channel: &str, vision: &str) {
    let listed = fx.call_ok("list_channels", json!({"projectRoot": fx.project}));
    let mut release = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == channel)
        .unwrap()
        .clone();
    let revision = release["revision"].clone();
    release["chief"] = json!(vision);
    fx.call_ok(
        "save_channel",
        json!({"channel": release, "revision": revision}),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_demoted_chief_no_longer_reads_its_workers_chats() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let (channel, juno, rook) = release_team(&fx);
    let (_, chief) = fx.vision_run(&juno, &channel).await;
    let work = fx.thread().await;
    fx.own(&work, &rook, &channel, OwnedKind::Work);
    assert!(fx
        .agent_call(&chief, "get_thread", json!({"threadId": work}))
        .status
        .success());
    make_chief(&fx, &channel, &rook);
    let refused = fx.agent_call(&chief, "get_thread", json!({"threadId": work}));
    assert!(!refused.status.success(), "{}", text(&refused));
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_promoted_worker_no_longer_drives_its_chats() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let provider = quick_provider().await;
    fx.use_provider(&provider).await;
    let (channel, _juno, rook) = release_team(&fx);
    let (_, worker) = fx.vision_run(&rook, &channel).await;
    let work = fx.vision_starts_chat(&worker);
    make_chief(&fx, &channel, &rook);
    let refused = fx.agent_call(&worker, "stop_thread", json!({"threadId": work}));
    assert!(!refused.status.success(), "{}", text(&refused));
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_team_pause_stays_in_its_channel_and_never_undoes_the_persons() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let (channel, juno, rook) = release_team(&fx);
    let (_, chief) = fx.vision_run(&juno, &channel).await;
    fx.agent_call_ok(&chief, "pause_worker", json!({"workerId": rook}));
    let team = fx.agent_call_ok(&chief, "list_team", json!({}));
    assert_eq!(team[0]["paused"], true, "{team}");
    // Work for a paused worker is refused, not left waiting where nothing would wake it.
    let tasked = fx.agent_call(
        &chief,
        "assign_task",
        json!({"workerId": rook, "task": "Fix the parser"}),
    );
    assert!(!tasked.status.success());
    assert!(
        String::from_utf8_lossy(&tasked.stderr).contains("paused"),
        "{}",
        String::from_utf8_lossy(&tasked.stderr)
    );
    let log = fx.call_ok(
        "get_team_log",
        json!({"channelId": channel, "workerId": rook}),
    );
    let kinds: Vec<_> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line["entry"]["kind"].clone())
        .collect();
    assert_eq!(kinds, vec![json!("paused")], "{log}");
    // Rook is not paused anywhere else.
    let rook_now = fx.call_ok("list_visions", json!({}));
    let rook_now = rook_now
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == rook.as_str())
        .unwrap();
    assert_eq!(rook_now["paused"], false, "{rook_now}");
    fx.agent_call_ok(&chief, "resume_worker", json!({"workerId": rook}));
    // The person's own pause is theirs: the chief's resume does not lift it.
    fx.call_ok("pause_vision", json!({"visionId": rook, "paused": true}));
    fx.agent_call_ok(&chief, "resume_worker", json!({"workerId": rook}));
    let rook_now = fx.call_ok("list_visions", json!({}));
    let rook_now = rook_now
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == rook.as_str())
        .unwrap();
    assert_eq!(rook_now["paused"], true, "{rook_now}");
    fx.shut_down().await;
}

/// The person reads a worker's thread with its chief, and the channel's spend today.
#[tokio::test(flavor = "multi_thread")]
async fn the_person_reads_a_workers_thread_and_the_channels_spend() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let (channel, juno, rook) = release_team(&fx);
    let (_, chief) = fx.vision_run(&juno, &channel).await;
    fx.agent_call_ok(
        &chief,
        "assign_task",
        json!({"workerId": rook, "task": "Fix the build"}),
    );
    let log = fx.call_ok(
        "get_team_log",
        json!({"channelId": channel, "workerId": rook}),
    );
    assert_eq!(log["entries"][0]["entry"]["text"], "Fix the build", "{log}");
    // Without prices, today's spend is unknown, never shown as nothing spent.
    let spend = fx.call_ok("get_channel_spend", json!({"channelId": channel}));
    assert!(spend["spent_usd"].is_null(), "{spend}");
    fx.state
        .set_model_catalog_for_test(vec![themis_core::providers::GoModel {
            id: "test-model".into(),
            price: Some(themis_core::providers::ModelPrice::default()),
            ..Default::default()
        }])
        .await;
    let spend = fx.call_ok("get_channel_spend", json!({"channelId": channel}));
    assert_eq!(spend["spent_usd"], 0.0, "{spend}");
    fx.shut_down().await;
}

/// Settings › Visions writes the team limits and quiet hours in your runtime.jsonc.
#[tokio::test(flavor = "multi_thread")]
async fn settings_write_the_team_limits_and_quiet_hours() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let read = fx.call_ok("get_visions_settings", json!({}));
    let changes = json!({"requests_per_day": 4, "routines_per_chief": 3,
        "quiet_hours": {"start": "22:00", "end": "07:00"}});
    let saved = fx.call_ok(
        "update_visions_settings",
        json!({"changes": changes, "revision": read["revision"]}),
    );
    assert_eq!(saved["settings"]["requests_per_day"], 4, "{saved}");
    assert_eq!(saved["settings"]["quiet_hours"]["start"], "22:00");
    // Quiet hours off again.
    let off = fx.call_ok(
        "update_visions_settings",
        json!({"changes": {"quiet_hours": null}, "revision": saved["revision"]}),
    );
    assert!(off["settings"]["quiet_hours"].is_null(), "{off}");
    let same = fx.call("update_visions_settings", json!({"changes": {"quiet_hours": {"start": "09:00", "end": "09:00"}}, "revision": off["revision"]}));
    assert!(!same.status.success());
    fx.shut_down().await;
}

/// The whole loop through the real server: the chief tasks a worker, the worker really runs, and
/// when its turn ends without doing anything (the model only says "on it", as a free model once
/// did) the server wakes the chief, whose own turn then speaks in the channel. Without that wake
/// the task would stall with nothing in the channel.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_does_nothing_brings_its_chief_back() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let provider = quick_provider().await;
    fx.use_provider(&provider).await;
    let (channel, juno, rook) = release_team(&fx);
    // The chief's credential, as its run would hold it (wakes stay on: the worker really runs).
    let chat = fx.thread().await;
    fx.own(&chat, &juno, &channel, OwnedKind::VisionChat);
    let mut scope = themis_core::agent_cli::RunScope::new(&fx.project, None);
    scope.vision = Some(fx.state.visions_scope(&juno, &channel).unwrap());
    let chief = themis_core::agent_cli::issue(&chat, "vision-run", &scope)
        .expect("issued")
        .env()[0]
        .1
        .clone();
    fx.agent_call_ok(
        &chief,
        "assign_task",
        json!({"workerId": rook, "task": "Write omelette.md"}),
    );
    wait_until(|| async {
        fx.entries(&channel)
            .iter()
            .any(|entry| entry["type"] == "vision" && entry["vision_id"] == juno.as_str())
    })
    .await;
    fx.shut_down().await;
}
