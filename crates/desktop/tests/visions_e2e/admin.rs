//! The admin Vision in `# administration` through the real app server: it proposes channels and Visions as a
//! proposal card and sets them up only after the person writes in `# administration` after it; a proposal
//! never raises autonomy or a budget and never deletes; its credential drives no chat; it and its
//! `# administration` cannot be deleted.

use themis_desktop::state::ChannelEntry;

use super::*;

const ADMIN: &str = "admin";

/// The project's `# administration` as listing the channels gives it (made on first listing, with the admin
/// as its only member).
fn hq(fx: &Fixture) -> String {
    let channels = fx.call_ok("list_channels", json!({"projectRoot": fx.project}));
    let hq = channels
        .as_array()
        .unwrap()
        .iter()
        .find(|channel| channel["name"] == "administration")
        .unwrap_or_else(|| panic!("no # administration in {channels}"))
        .clone();
    assert_eq!(hq["members"], json!([ADMIN]), "{hq}");
    hq["id"].as_str().unwrap().to_owned()
}

fn named<'a>(list: &'a Value, name: &str) -> Option<&'a Value> {
    list.as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == name)
}

fn channels(fx: &Fixture) -> Value {
    fx.call_ok("list_channels", json!({"projectRoot": fx.project}))
}

fn visions(fx: &Fixture) -> Value {
    fx.call_ok("list_visions", json!({}))
}

/// A channel that ships releases, run by a new Vision, Rook.
fn releases() -> Value {
    json!({
        "summary": "A channel for shipping releases, with Rook to run it.",
        "channels": [{"name": "releases", "topic": "Ship releases", "members": ["Rook"]}],
        "visions": [{"name": "Rook", "role": "Ships releases"}]
    })
}

fn propose(fx: &Fixture, admin: &str, channel: &str, proposal: Value) -> std::process::Output {
    let args = json!({"channelId": channel, "proposal": proposal});
    fx.agent_call(admin, "propose_visions_setup", args)
}

fn apply(fx: &Fixture, admin: &str, proposal: &str) -> std::process::Output {
    fx.agent_call(admin, "apply_proposal", json!({"proposalId": proposal}))
}

fn proposal_id(output: &std::process::Output) -> String {
    json_of(output)["proposalId"].as_str().unwrap().to_owned()
}

fn say(fx: &Fixture, channel: &str, text: &str) {
    fx.call_ok(
        "post_channel_message",
        json!({"channelId": channel, "text": text}),
    );
}

fn logs(fx: &Fixture) -> String {
    std::fs::read_dir(fx.data.join("logs"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_admin_proposes_and_creates_only_after_the_person_replies() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let hq = hq(&fx);
    let atlas = named(&visions(&fx), "Atlas")
        .cloned()
        .expect("Atlas is listed");
    assert_eq!(atlas["id"], ADMIN);
    let (_, admin) = fx.vision_run(ADMIN, &hq).await;
    say(&fx, &hq, "I need a channel that ships releases");

    let proposed = propose(&fx, &admin, &hq, releases());
    let id = proposal_id(&proposed);
    let card = fx.entries(&hq).last().cloned().unwrap();
    assert_eq!(card["type"], "proposal", "{card}");
    assert_eq!(card["vision_id"], ADMIN);
    assert_eq!(card["proposal_id"], id.as_str());
    assert_eq!(card["proposal"]["summary"], releases()["summary"]);
    // The server gives the new Vision its colour, so the card can draw it (a Vision has no shape).
    let rook = &card["proposal"]["visions"][0];
    assert_eq!(rook["name"], "Rook");
    assert!(rook.get("shape").is_none(), "{rook}");
    assert!(rook["color"]
        .as_str()
        .is_some_and(|color| !color.is_empty()));
    assert!(named(&channels(&fx), "releases").is_none());
    assert!(named(&visions(&fx), "Rook").is_none());

    // The person has not written since the proposal: nothing is set up.
    let early = apply(&fx, &admin, &id);
    assert!(failed_with(&early, "has not replied"), "{}", text(&early));
    assert!(named(&visions(&fx), "Rook").is_none());

    say(&fx, &hq, "Yes, go ahead");
    let applied = json_of(&apply(&fx, &admin, &id));
    let rook = named(&visions(&fx), "Rook")
        .cloned()
        .expect("Rook was created");
    assert_eq!(rook["role"], "Ships releases");
    let made = named(&channels(&fx), "releases")
        .cloned()
        .expect("# releases");
    assert_eq!(made["members"], json!([rook["id"]]));
    assert_eq!(made["full_access"], false);
    assert_eq!(made["topic"], "Ship releases");
    let items = json!([
        {"kind": "vision", "id": rook["id"], "name": "Rook", "action": "created"},
        {"kind": "channel", "id": made["id"], "name": "releases", "action": "created"}
    ]);
    assert_eq!(applied["items"], items);
    let done = fx.entries(&hq).last().cloned().unwrap();
    assert_eq!(
        done,
        json!({"type": "proposal_applied", "vision_id": ADMIN, "proposal_id": id, "items": items})
    );

    // Once applied, never again.
    let again = apply(&fx, &admin, &id);
    assert!(failed_with(&again, "already set up"), "{}", text(&again));
    assert_eq!(fx.entries(&hq).len(), 4);
    let logs = logs(&fx);
    assert!(logs.contains("\"proposal_posted\""), "{logs}");
    assert!(logs.contains("\"proposal_applied\""), "{logs}");
    for words in ["shipping releases", "Ships releases", "go ahead"] {
        assert!(!logs.contains(words), "the log kept {words:?}");
    }
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn apply_is_refused_without_a_person_message_after_the_proposal() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let hq = hq(&fx);
    let (_, admin) = fx.vision_run(ADMIN, &hq).await;
    // The person wrote before the proposal, and only the admin after it.
    say(&fx, &hq, "Set up releases, yes to anything");
    let id = proposal_id(&propose(&fx, &admin, &hq, releases()));
    fx.state
        .channel_append(
            &hq,
            ChannelEntry::Vision {
                vision_id: ADMIN.into(),
                text: "Shall I?".into(),
                tasked: Vec::new(),
            },
        )
        .unwrap();
    let refused = apply(&fx, &admin, &id);
    assert!(
        failed_with(&refused, "has not replied"),
        "{}",
        text(&refused)
    );
    let unknown = apply(&fx, &admin, "no-such-proposal");
    assert!(
        failed_with(&unknown, "Unknown proposal"),
        "{}",
        text(&unknown)
    );
    // Nothing was set up, and the log has the refusal's reason only.
    assert_eq!(channels(&fx).as_array().unwrap().len(), 1);
    assert_eq!(visions(&fx).as_array().unwrap().len(), 1);
    let logs = logs(&fx);
    assert!(logs.contains("\"no_reply\""), "{logs}");
    assert!(!logs.contains("yes to anything"));
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_proposal_cannot_raise_autonomy_or_budget_or_delete() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let hq = hq(&fx);
    fx.set_visions(json!({"enabled": false, "autonomy": "small", "daily_budget_usd": 1.0}));
    let (wren, _) = fx.vision_and_channel("Wren", "work");
    let saved = visions(&fx);
    let revision = named(&saved, "Wren").unwrap()["revision"].clone();
    fx.call_ok(
        "save_vision",
        json!({"vision": {"id": wren, "name": "Wren", "color": "teal",
            }, "revision": revision}),
    );
    fx.state
        .set_model_catalog_for_test(vec![themis_core::providers::GoModel {
            id: "test-model".into(),
            ..Default::default()
        }])
        .await;
    let (_, admin) = fx.vision_run(ADMIN, &hq).await;
    let with = |field: &str, value: Value| {
        let mut proposal = json!({"summary": "A change."});
        proposal[field] = value;
        proposal
    };
    let refused = [
        (
            with("visions", json!([{"name": "Rook", "autonomy": "most"}])),
            "autonomy",
        ),
        // Settings allow Small, but Wren's own level is Ask me first: a change only lowers it.
        (
            with("changes", json!([{"vision": "Wren", "autonomy": "small"}])),
            "autonomy",
        ),
        (
            with(
                "visions",
                json!([{"name": "Rook", "model": "no-such-model"}]),
            ),
            "model",
        ),
        (
            with(
                "changes",
                json!([{"vision": "Wren", "autonomy": "nearly_all"}]),
            ),
            "autonomy",
        ),
        (
            with(
                "visions",
                json!([{"name": "Rook", "daily_budget_usd": 5.0}]),
            ),
            "budget",
        ),
        (
            with(
                "changes",
                json!([{"vision": "Wren", "daily_budget_usd": 0.75}]),
            ),
            "budget",
        ),
        (
            with("changes", json!([{"vision": "Wren", "delete": true}])),
            "delete",
        ),
        (
            with("changes", json!([{"channel": "work", "delete": true}])),
            "delete",
        ),
        (
            with("changes", json!([{"vision": "Atlas", "name": "Boss"}])),
            "Atlas",
        ),
    ];
    for (proposal, words) in refused {
        let output = propose(&fx, &admin, &hq, proposal.clone());
        assert!(failed_with(&output, words), "{proposal}: {}", text(&output));
    }
    assert!(
        fx.entries(&hq).is_empty(),
        "a refused proposal posts nothing"
    );

    // Autonomy and budget are the team's now: Task 6 checks them on a team proposal.
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_admin_cannot_drive_chats() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let hq = hq(&fx);
    let (wren, work) = fx.chief_and_channel("Wren", "work");
    let chat = fx.thread().await;
    fx.own(&chat, &wren, &work, OwnedKind::Work);
    let (own, admin) = fx.vision_run(ADMIN, &hq).await;
    let calls = [
        (
            "create_thread",
            json!({"projectRoot": fx.project, "provider": "go"}),
        ),
        ("send_message", json!({"threadId": chat, "text": "go"})),
        ("send_message", json!({"threadId": own, "text": "go"})),
        ("enqueue_message", json!({"threadId": chat, "text": "go"})),
        ("stop_thread", json!({"threadId": chat})),
        (
            "answer_question",
            json!({"threadId": chat, "questionId": "q", "answer": "a", "decision": "wording",
                "reason": "r"}),
        ),
        (
            "pass_question",
            json!({"threadId": chat, "questionId": "q", "decision": "wording"}),
        ),
        ("list_channel_threads", json!({"channelId": hq})),
        (
            "save_vision",
            json!({"vision": {"name": "X", "color": "blue"}}),
        ),
        ("delete_vision", json!({"visionId": wren})),
        ("plugin_action", json!({"action": "list"})),
        (
            "propose_visions_setup",
            json!({"channelId": work, "proposal": releases()}),
        ),
    ];
    for (method, args) in calls {
        let output = fx.agent_call(&admin, method, args);
        let refused = format!("Themis refused `{method}`");
        assert!(
            failed_with(&output, &refused),
            "{method}: {}",
            text(&output)
        );
    }
    // It reads the Visions and its project's channels.
    assert!(named(&fx.agent_call_ok(&admin, "list_visions", json!({})), "Wren").is_some());
    let listed = fx.agent_call_ok(&admin, "list_channels", json!({"projectRoot": fx.project}));
    assert!(named(&listed, "work").is_some());
    // Only the admin proposes or applies.
    let (_, wren_run) = fx.vision_run(&wren, &work).await;
    for method in ["propose_visions_setup", "apply_proposal"] {
        let args = json!({"channelId": hq, "proposal": releases(), "proposalId": "p"});
        let output = fx.agent_call(&wren_run, method, args);
        let refused = format!("Themis refused `{method}`");
        assert!(
            failed_with(&output, &refused),
            "{method}: {}",
            text(&output)
        );
    }
    // And the person cannot set up a proposal with their own credential.
    let person = fx.call("apply_proposal", json!({"proposalId": "p"}));
    assert!(!person.status.success());
    assert!(fx.entries(&hq).is_empty());
    assert!(fx
        .state
        .owner_of(&chat)
        .is_some_and(|o| o.vision_id == wren));
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_admin_cannot_be_deleted() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let hq = hq(&fx);
    // Atlas's own chat in # administration stays its own through the refused deletes.
    let (own, _) = fx.vision_run(ADMIN, &hq).await;
    let gone = fx.call("delete_vision", json!({"visionId": ADMIN}));
    assert!(failed_with(&gone, "built in"), "{}", text(&gone));
    assert!(named(&visions(&fx), "Atlas").is_some());
    let gone = fx.call("delete_channel", json!({"channelId": hq}));
    assert!(failed_with(&gone, "built in"), "{}", text(&gone));
    assert!(
        fx.state.owner_of(&own).is_some(),
        "a refused delete released nothing"
    );
    // `# administration` keeps the admin as its only member, and the admin works nowhere else.
    let (wren, work) = fx.chief_and_channel("Wren", "work");
    let listed = channels(&fx);
    let hq_now = named(&listed, "administration").unwrap();
    let crowded = json!({"id": hq, "name": "administration", "project_root": fx.project,
        "autonomy": "ask", "members": [ADMIN, wren]});
    let saved = fx.call(
        "save_channel",
        json!({"channel": crowded, "revision": hq_now["revision"]}),
    );
    assert!(failed_with(&saved, "only member"), "{}", text(&saved));
    let work_now = named(&listed, "work").unwrap();
    let joined = json!({"id": work, "name": "work", "project_root": fx.project,
        "autonomy": "ask", "members": [wren, ADMIN]});
    let saved = fx.call(
        "save_channel",
        json!({"channel": joined, "revision": work_now["revision"]}),
    );
    assert!(failed_with(&saved, "only member"), "{}", text(&saved));
    assert_eq!(
        named(&channels(&fx), "administration").unwrap()["members"],
        json!([ADMIN])
    );
    // Each project lists its own `# administration`, all with the one admin.
    let (_other_repo, other) = crate::common::init_repo();
    let theirs = fx.call_ok("list_channels", json!({"projectRoot": other}));
    let theirs = named(&theirs, "administration").unwrap();
    assert_eq!(theirs["members"], json!([ADMIN]));
    assert_ne!(theirs["id"], hq.as_str());
    assert_eq!(visions(&fx).as_array().unwrap().len(), 2);
    fx.shut_down().await;
}

/// A setup's changes go through the server: a Vision's new role and pause (its channels say it
/// was paused) and a channel's members, each listed as changed.
#[tokio::test(flavor = "multi_thread")]
async fn the_admin_applies_changes_to_visions_and_channels() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let hq = hq(&fx);
    let (wren, work) = fx.chief_and_channel("Wren", "work");
    let (_, admin) = fx.vision_run(ADMIN, &hq).await;
    let proposal = json!({
        "summary": "Rook takes over the work channel; Wren rests.",
        "visions": [{"name": "Rook", "role": "Does the work"}],
        "changes": [
            {"vision": "Wren", "role": "Writes notes", "paused": true},
            {"channel": "work", "topic": "The work", "add_members": ["Rook"], "remove_members": ["Wren"]}
        ]
    });
    let id = proposal_id(&propose(&fx, &admin, &hq, proposal));
    say(&fx, &hq, "yes please");
    let applied = json_of(&apply(&fx, &admin, &id));
    let listed = visions(&fx);
    let rook = named(&listed, "Rook").unwrap()["id"].clone();
    let wren_now = named(&listed, "Wren").unwrap();
    assert_eq!(wren_now["role"], "Writes notes");
    assert_eq!(wren_now["paused"], true);
    let work_now = named(&channels(&fx), "work").cloned().unwrap();
    assert_eq!(work_now["members"], json!([rook]));
    assert_eq!(work_now["topic"], "The work");
    assert_eq!(
        applied["items"],
        json!([
            {"kind": "vision", "id": rook, "name": "Rook", "action": "created"},
            {"kind": "vision", "id": wren, "name": "Wren", "action": "changed"},
            {"kind": "channel", "id": work, "name": "work", "action": "changed"}
        ])
    );
    let paused = fx
        .entries(&work)
        .into_iter()
        .any(|entry| entry["type"] == "notice" && entry["kind"] == "paused");
    assert!(paused, "the pause went through pause_vision's path");
    assert_eq!(fx.entries(&hq).last().unwrap()["items"], applied["items"]);
    fx.shut_down().await;
}

/// The first listing of a project's channels makes Atlas and `# administration` and says so, so an app that
/// read the Visions first reads them again and finds Atlas.
#[tokio::test(flavor = "multi_thread")]
async fn making_hq_announces_a_visions_change() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let mut events = Client::new(fx.data.clone()).subscribe().await.unwrap();
    assert_eq!(visions(&fx), json!([]));
    hq(&fx);
    let changed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = Client::next_event(&mut events).await.unwrap();
            if event["name"] == "visions-changed" {
                break event;
            }
        }
    })
    .await
    .expect("making # administration announces visions-changed");
    assert_eq!(changed["payload"], json!({}), "{changed}");
    assert!(named(&visions(&fx), "Atlas").is_some());
    fx.shut_down().await;
}

/// The person's post says which Visions it woke (a run started or queued), so the channel can
/// show them thinking before their run starts; none while Visions are off.
#[tokio::test(flavor = "multi_thread")]
async fn a_post_says_which_visions_it_woke() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let provider = quick_provider().await;
    fx.use_provider(&provider).await;
    let hq = hq(&fx);
    let posted = fx.call_ok(
        "post_channel_message",
        json!({"channelId": hq, "text": "I need a channel for releases"}),
    );
    assert_eq!(posted["woken"], json!([ADMIN]), "{posted}");
    // Atlas answers, so its run is over before the next post.
    wait_until(|| async {
        fx.entries(&hq)
            .iter()
            .any(|entry| entry["type"] == "vision")
    })
    .await;
    fx.no_wakes();
    let quiet = fx.call_ok(
        "post_channel_message",
        json!({"channelId": hq, "text": "never mind"}),
    );
    assert_eq!(quiet["woken"], json!([]), "{quiet}");
    fx.shut_down().await;
}

/// A whole team, as Atlas proposes it after the person's yes: the channel, its chief (with its
/// soul) and workers (with their roles), and the team's autonomy and budget, nothing else.
#[tokio::test(flavor = "multi_thread")]
async fn a_team_proposal_creates_exactly_what_it_says() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let hq = hq(&fx);
    let (_, admin) = fx.vision_run(ADMIN, &hq).await;
    let team = json!({"summary": "A release team.", "team": {
        "channel": {"name": "release", "topic": "Ship 0.9"},
        "chief": {"name": "Juno", "soul": "Calm and direct.", "accessory": "glasses"},
        "workers": [{"name": "Rook", "role": "Builds"}, {"name": "Tess", "role": "Tests", "accessory": "magnifier"}],
        "autonomy": "ask", "daily_budget_usd": 0.5}});
    let id = proposal_id(&propose(&fx, &admin, &hq, team));
    say(&fx, &hq, "yes, go ahead");
    let applied = json_of(&apply(&fx, &admin, &id));
    assert_eq!(applied["items"].as_array().unwrap().len(), 4, "{applied}");
    let listed = visions(&fx);
    let juno = named(&listed, "Juno").unwrap();
    assert_eq!(juno["soul"], "Calm and direct.");
    assert_eq!(juno["accessory"], "glasses");
    assert_eq!(named(&listed, "Tess").unwrap()["role"], "Tests");
    let release = named(&channels(&fx), "release").cloned().unwrap();
    assert_eq!(release["chief"], juno["id"]);
    assert_eq!(release["members"].as_array().unwrap().len(), 3);
    assert_eq!(release["autonomy"], "ask");
    assert_eq!(release["daily_budget_usd"], 0.5);
    assert_eq!(release["topic"], "Ship 0.9");
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hire_needs_the_persons_yes() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let hq = hq(&fx);
    let (_, docs) = fx.chief_and_channel("Wren", "docs");
    let (_, admin) = fx.vision_run(ADMIN, &hq).await;
    let hire = json!({"summary": "A tester for # docs.", "hire": {"channel": "docs", "worker": {"name": "Tess", "role": "Tests"}}});
    let id = proposal_id(&propose(&fx, &admin, &hq, hire));
    assert!(!apply(&fx, &admin, &id).status.success(), "no yes yet");
    say(&fx, &hq, "yes");
    json_of(&apply(&fx, &admin, &id));
    let tess = named(&visions(&fx), "Tess").unwrap()["id"].clone();
    let docs_now = channels(&fx)
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == docs.as_str())
        .cloned()
        .unwrap();
    assert!(
        docs_now["members"].as_array().unwrap().contains(&tess),
        "{docs_now}"
    );
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chief_asking_for_a_worker_wakes_atlas_and_shows_in_administration() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let hq = hq(&fx);
    let (wren, docs) = fx.chief_and_channel("Wren", "docs");
    let (_, chief) = fx.vision_run(&wren, &docs).await;
    let asked = fx.agent_call_ok(
        &chief,
        "ask_for_worker",
        json!({"role": "Tests", "why": "Releases keep breaking"}),
    );
    assert!(asked["requestId"].is_string(), "{asked}");
    let line = fx
        .entries(&hq)
        .into_iter()
        .find(|e| e["type"] == "worker_asked")
        .expect("in # administration");
    assert_eq!(line["vision_id"], wren.as_str());
    assert_eq!(line["channel_id"], docs.as_str());
    assert_eq!(line["role"], "Tests");
    // It counts towards the channel's daily requests.
    fx.set_visions(json!({"enabled": false, "requests_per_day": 1}));
    let again = fx.agent_call(
        &chief,
        "ask_for_worker",
        json!({"role": "Docs", "why": "x"}),
    );
    assert!(!again.status.success());
    fx.shut_down().await;
}
