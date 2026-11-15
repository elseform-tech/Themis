use std::path::PathBuf;

use super::*;
use crate::state::visions::channel_log::take_bytes_read;

/// An absolute project folder on every OS (Windows needs a drive).
const ROOT: &str = if cfg!(windows) {
    r"C:\work\app"
} else {
    "/work/app"
};

fn vision(id: &str, name: &str, _budget: Option<f64>) -> Vision {
    Vision {
        id: id.into(),
        name: name.into(),
        accessory: "none".into(),
        ..admin_profile()
    }
}

fn channel(id: &str, name: &str, members: &[&str]) -> Channel {
    Channel {
        id: id.into(),
        name: name.into(),
        topic: String::new(),
        project_root: PathBuf::from(ROOT),
        full_access: false,
        members: members.iter().map(|member| (*member).to_owned()).collect(),
        chief: members
            .first()
            .map(|member| (*member).to_owned())
            .unwrap_or_default(),
        autonomy: None,
        custom: None,
        daily_budget_usd: None,
        paused_workers: Vec::new(),
        revision: "r".into(),
    }
}

pub(super) fn context() -> Context {
    Context {
        visions: vec![admin_profile(), vision("w", "Wren", Some(0.5))],
        channels: vec![
            channel("h", ADMINISTRATION, &[ADMIN_VISION_ID]),
            channel("d", "docs", &["w"]),
        ],
        config: VisionsConfig::default(),
        models: vec!["test-model".into()],
    }
}

pub(super) fn proposal(json: serde_json::Value) -> Proposal {
    let mut proposal: Proposal = serde_json::from_value(json).unwrap();
    fill_looks(&mut proposal, 2);
    proposal
}

#[test]
fn a_proposal_is_checked_field_by_field() {
    let ok = |json| check(&proposal(json), &context());
    let fine = serde_json::json!({"summary": "Releases", "visions": [{"name": "Rook"}],
        "channels": [{"name": "releases", "members": ["rook", "Wren"]}],
        "changes": [{"channel": "docs", "add_members": ["Rook"], "topic": "Docs"},
            {"vision": "wren", "role": "Writes"}]});
    assert_eq!(ok(fine), Ok(()));
    let refused = [
        (
            serde_json::json!({"summary": "", "visions": [{"name": "Rook"}]}),
            "summary",
        ),
        (serde_json::json!({"summary": "Nothing"}), "1 to 16"),
        (
            serde_json::json!({"summary": "x", "visions": [{"name": "wren"}]}),
            "already exists",
        ),
        (
            serde_json::json!({"summary": "x", "visions": [{"name": "Rook", "autonomy": "most"}]}),
            "belong to its channel",
        ),
        (
            serde_json::json!({"summary": "x", "visions": [{"name": "Rook", "shape": "square"}]}),
            "shape",
        ),
        (
            serde_json::json!({"summary": "x", "visions": [{"name": "Rook", "daily_budget_usd": 1.5}]}),
            "budget",
        ),
        (
            serde_json::json!({"summary": "x", "changes": [{"vision": "Wren", "daily_budget_usd": 0.6}]}),
            "budget",
        ),
        (
            serde_json::json!({"summary": "x", "channels": [{"name": "administration"}]}),
            "# administration",
        ),
        (
            serde_json::json!({"summary": "x", "channels": [{"name": "Big Room"}]}),
            "lowercase",
        ),
        (
            serde_json::json!({"summary": "x", "channels": [{"name": "ops", "members": ["Atlas"]}]}),
            "only in # administration",
        ),
        (
            serde_json::json!({"summary": "x", "channels": [{"name": "ops", "members": ["Nobody"]}]}),
            "No Vision",
        ),
        (
            serde_json::json!({"summary": "x", "changes": [{"channel": "administration", "topic": "Mine"}]}),
            "# administration is built in",
        ),
        (
            serde_json::json!({"summary": "x", "changes": [{"channel": "docs", "add_members": ["Atlas"]}]}),
            "only in # administration",
        ),
        (
            serde_json::json!({"summary": "x", "changes": [{"channel": "docs", "name": "administration"}]}),
            "# administration",
        ),
        (
            serde_json::json!({"summary": "x", "changes": [{"vision": "Atlas", "role": "Boss"}]}),
            "Atlas",
        ),
        (
            serde_json::json!({"summary": "x", "changes": [{"vision": "Wren"}]}),
            "changes nothing",
        ),
        (
            serde_json::json!({"summary": "x", "changes": [{"vision": "Wren", "topic": "x"}]}),
            "name, role",
        ),
        (
            serde_json::json!({"summary": "x", "changes": [{"channel": "docs", "paused": true}]}),
            "name, topic",
        ),
        (
            serde_json::json!({"summary": "x", "changes": [{"vision": "Wren", "channel": "docs"}]}),
            "either one",
        ),
    ];
    for (json, words) in refused {
        let error = ok(json.clone()).expect_err(&json.to_string());
        assert!(error.contains(words), "{json}: {error}");
    }
    // Nothing deletes: there is no such field.
    let delete = serde_json::from_value::<Proposal>(
        serde_json::json!({"summary": "x", "changes": [{"vision": "Wren", "delete": true}]}),
    );
    assert!(delete.unwrap_err().to_string().contains("delete"));
}

/// Autonomy and budget are the channel's now: a proposal cannot set them on a Vision.
#[test]
fn a_proposal_cannot_set_autonomy_or_budget_on_a_vision() {
    for json in [
        serde_json::json!({"summary": "x", "changes": [{"vision": "Wren", "autonomy": "ask"}]}),
        serde_json::json!({"summary": "x", "changes": [{"vision": "Wren", "daily_budget_usd": 0.2}]}),
        serde_json::json!({"summary": "x", "visions": [{"name": "Rook", "autonomy": "ask"}]}),
        serde_json::json!({"summary": "x", "visions": [{"name": "Rook", "daily_budget_usd": 0.2}]}),
    ] {
        let error = check(&proposal(json.clone()), &context()).unwrap_err();
        assert!(error.contains("belong to its channel"), "{json}: {error}");
    }
}

#[test]
fn new_visions_get_a_look_in_turn_and_keep_their_own() {
    let filled = proposal(serde_json::json!({"summary": "x",
        "visions": [{"name": "A"}, {"name": "B", "shape": "wide", "color": "rose"}, {"name": "C"}]}));
    let looks: Vec<(&str, &str)> = filled
        .visions
        .iter()
        .map(|v| (v.shape.as_str(), v.color.as_str()))
        .collect();
    assert_eq!(
        looks,
        [("two-bump", "violet"), ("wide", "rose"), ("wide", "rose")]
    );
}

/// The admin's run has its own head: the propose and apply calls with this `# administration`'s ids, the
/// plain voice, and asking before setting anything up.
#[test]
fn the_admin_runs_on_its_own_head() {
    let atlas = Vision {
        soul: "Tidy and brief.".into(),
        ..admin_profile()
    };
    let head = run_head(
        &atlas,
        &channel("h1", ADMINISTRATION, &[ADMIN_VISION_ID]),
        &VisionsConfig::default(),
        &[],
    );
    println!("{head}");
    assert!(head.starts_with("You are Atlas"));
    assert!(head.contains(r#"propose_visions_setup '{"channelId": "h1""#));
    assert!(head.contains(r#"apply_proposal '{"proposalId": "ID"}'"#));
    let listed = format!(
        "list_channels '{{\"projectRoot\": {}}}'",
        serde_json::to_string(ROOT).unwrap()
    );
    assert!(head.contains(&listed), "{listed}");
    assert!(head.contains("one proposal per message"));
    assert!(head.contains("clear yes"));
    assert!(head.contains("only when the person asks for one"));
    assert!(head.contains("Your shell runs only `themis`"));
    assert!(!head.contains("create_thread"), "it drives no chat");
    assert!(head.contains("Tidy and brief."), "its soul");
    assert!(head.contains(r#""team": {"channel""#), "it proposes teams");
    // Any other Vision keeps the ordinary head.
    let wren = vision("w", "Wren", None);
    let other = run_head(
        &wren,
        &channel("d", "docs", &["w"]),
        &VisionsConfig::default(),
        std::slice::from_ref(&wren),
    );
    assert!(other.starts_with("You are Wren, the chief of staff of #docs"));
}

/// An app with a project's `# administration` (and the admin), and that channel's id.
fn with_hq() -> (tempfile::TempDir, AppState, String) {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let root = dir.path().canonicalize().unwrap();
    state.visions_ensure_administration(&root).unwrap();
    let hq = state.channel_list(&root).remove(0).id;
    (dir, state, hq)
}

/// Looking a proposal up reads `# administration` back from its end only so far ([`SCAN_BYTES`]),
/// however long the log: the work stays flat as the log doubles, and an older proposal is
/// unknown (proposed again).
#[test]
fn finding_a_proposal_reads_a_bounded_tail() {
    let samples = crate::scale::measure(&[150, 300], |size| {
        let (_dir, state, hq) = with_hq();
        let proposal = proposal(serde_json::json!({"summary": "x", "visions": [{"name": "Rook"}]}));
        let entry = ChannelEntry::Proposal {
            vision_id: ADMIN_VISION_ID.into(),
            proposal_id: "p1".into(),
            proposal,
        };
        state.channel_append(&hq, entry).unwrap();
        let text = "word ".repeat(1000);
        for _ in 0..size {
            let entry = ChannelEntry::Person { text: text.clone() };
            state.channel_append(&hq, entry).unwrap();
        }
        take_bytes_read();
        let error = state.visions_find_proposal(&hq, "p1").err().unwrap();
        assert!(error.contains("Unknown proposal"), "{error}");
        take_bytes_read()
    });
    let rate = crate::scale::growth_rate(&samples);
    assert!(
        rate.abs() < 0.1,
        "the look-up grew with the log: {samples:?}"
    );
}

/// Only the admin's newest proposal in `# administration` can be set up: the person turned A down, the
/// admin posted B, and the person's "yes" is to B, so applying A is refused and B applies.
#[test]
fn only_the_newest_proposal_can_be_set_up() {
    let (_dir, state, hq) = with_hq();
    let propose = |name: &str| {
        let spec = proposal(serde_json::json!({"summary": name, "visions": [{"name": name}]}));
        state.visions_propose(&hq, spec, Vec::new()).unwrap().0
    };
    let person = |text: &str| {
        let entry = ChannelEntry::Person { text: text.into() };
        state.channel_append(&hq, entry).unwrap();
    };
    let first = propose("Rook");
    person("no, not Rook");
    let second = propose("Wren");
    person("yes");
    let refused = state
        .visions_apply_proposal(&hq, &first, Vec::new())
        .unwrap_err();
    assert_eq!(refused, themis_core::prompts::AGENT_CLI_ADMIN_SUPERSEDED);
    let names = || -> Vec<String> { state.visions_list().into_iter().map(|v| v.name).collect() };
    assert!(!names().contains(&"Rook".to_owned()), "{:?}", names());
    let items = state
        .visions_apply_proposal(&hq, &second, Vec::new())
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "Wren");
}

/// Listing makes the admin once and each project's `# administration` once, and gives an `# administration` saved with
/// other members back to the admin alone.
#[test]
fn hq_is_made_once_per_project_with_the_admin_alone() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let root = dir.path().canonicalize().unwrap();
    state.visions_ensure_administration(&root).unwrap();
    state.visions_ensure_administration(&root).unwrap();
    let listed = state.channel_list(&root);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].members, [ADMIN_VISION_ID]);
    assert_eq!(
        state.visions_list(),
        vec![{
            let mut admin = admin_profile();
            admin.revision = state.vision_get(ADMIN_VISION_ID).unwrap().revision;
            admin
        }]
    );
    let wren = state.vision_save(vision("", "Wren", None), None).unwrap();
    // An `# administration` written by hand with another member.
    let mut cache = state.visions_cache();
    let channels = cache.channels(&state.inner.visions_dir).unwrap();
    let hq = channels.values_mut().next().unwrap();
    hq.members = vec![wren.id.clone()];
    drop(cache);
    state.visions_ensure_administration(&root).unwrap();
    assert_eq!(state.channel_list(&root)[0].members, [ADMIN_VISION_ID]);
}

/// The models a proposal may name are the Go catalog's and the configured providers' (only by
/// the qualified `<provider>/<model>` id, which routes a chat there), also with no Go key.
#[tokio::test]
async fn configured_provider_models_are_usable_without_a_go_key() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let runtime = serde_json::json!({"providers": [{"id": "local", "name": "Local",
        "base_url": "http://127.0.0.1:9/v1", "secret": "${LOCAL_KEY}",
        "models": [{"id": "qwen"}]}]});
    std::fs::write(dir.path().join("runtime.jsonc"), runtime.to_string()).unwrap();
    let models = state.visions_usable_models().await;
    let named = |model: &str| {
        let json =
            serde_json::json!({"summary": "x", "visions": [{"name": "Rook", "model": model}]});
        check(
            &proposal(json),
            &Context {
                models: models.clone(),
                ..context()
            },
        )
    };
    assert_eq!(named("local/qwen"), Ok(()), "{models:?}");
    // A bare id would run as an OpenCode Go model: only the qualified one routes there.
    assert!(named("qwen").unwrap_err().contains("model"));
    assert!(named("local/nope").unwrap_err().contains("model"));
    assert!(named("no-such-model").unwrap_err().contains("model"));
}

/// The log keeps a proposal id only when it is id-shaped: the admin's call names it, so any
/// other text (words, a secret pasted in) is logged as `(other)`.
#[test]
fn the_log_keeps_proposal_ids_only_when_id_shaped() {
    let (dir, state, hq) = with_hq();
    let named = "set up my plan with sk-secret-1234";
    let refused = state.visions_apply_proposal(&hq, named, Vec::new());
    assert!(refused.is_err());
    let mut logged = String::new();
    for entry in std::fs::read_dir(dir.path().join("logs"))
        .unwrap()
        .flatten()
    {
        logged.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
    }
    let refusals: Vec<serde_json::Value> = logged
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|line| line["event"] == "proposal_refused")
        .collect();
    assert_eq!(refusals.len(), 1, "{logged}");
    assert_eq!(refusals[0]["details"]["proposal_id"], "(other)");
    assert!(!logged.contains("sk-secret-1234"), "{logged}");
}
