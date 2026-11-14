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
