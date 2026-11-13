use std::path::PathBuf;

use super::*;
use crate::state::visions::channel_log::take_bytes_read;

fn vision(id: &str, name: &str, budget: Option<f64>) -> Vision {
    Vision {
        id: id.into(),
        daily_budget_usd: budget,
        name: name.into(),
        ..admin_profile()
    }
}

fn channel(id: &str, name: &str, members: &[&str]) -> Channel {
    Channel {
        id: id.into(),
        name: name.into(),
        topic: String::new(),
        project_root: PathBuf::from("/work/app"),
        approval_mode: ApprovalMode::Custom,
        members: members.iter().map(|member| (*member).to_owned()).collect(),
        revision: "r".into(),
    }
}

fn context() -> Context {
    Context {
        visions: vec![admin_profile(), vision("w", "Wren", Some(0.5))],
        channels: vec![
            channel("h", HQ, &[ADMIN_VISION_ID]),
            channel("d", "docs", &["w"]),
        ],
        config: VisionsConfig::default(),
    }
}

fn proposal(json: serde_json::Value) -> Proposal {
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
            {"vision": "wren", "role": "Writes", "autonomy": "ask", "daily_budget_usd": 0.5}]});
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
            "autonomy",
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
            serde_json::json!({"summary": "x", "channels": [{"name": "hq"}]}),
            "# hq",
        ),
        (
            serde_json::json!({"summary": "x", "channels": [{"name": "Big Room"}]}),
            "lowercase",
        ),
        (
            serde_json::json!({"summary": "x", "channels": [{"name": "ops", "members": ["Atlas"]}]}),
            "only in # hq",
        ),
        (
            serde_json::json!({"summary": "x", "channels": [{"name": "ops", "members": ["Nobody"]}]}),
            "No Vision",
        ),
        (
            serde_json::json!({"summary": "x", "changes": [{"channel": "hq", "topic": "Mine"}]}),
            "# hq is built in",
        ),
        (
            serde_json::json!({"summary": "x", "changes": [{"channel": "docs", "add_members": ["Atlas"]}]}),
            "only in # hq",
        ),
        (
            serde_json::json!({"summary": "x", "changes": [{"channel": "docs", "name": "hq"}]}),
            "# hq",
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

/// At Custom, a proposal's level may decide only what the custom rows already allow.
#[test]
fn a_proposals_autonomy_never_passes_the_resolved_level() {
    let custom = VisionsConfig {
        autonomy: Autonomy::Custom,
        custom: [("tests".to_owned(), true)].into(),
        ..VisionsConfig::default()
    };
    assert!(autonomy_fits(&custom, Autonomy::Ask).is_ok());
    assert!(autonomy_fits(&custom, Autonomy::Custom).is_ok());
    assert!(
        autonomy_fits(&custom, Autonomy::Small).is_err(),
        "small adds wording"
    );
    let most = VisionsConfig {
        autonomy: Autonomy::Most,
        ..VisionsConfig::default()
    };
    assert!(autonomy_fits(&most, Autonomy::Most).is_ok());
    assert!(autonomy_fits(&most, Autonomy::NearlyAll).is_err());
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

/// The admin's run has its own head: the propose and apply calls with this `# hq`'s ids, the
/// plain voice, and asking before setting anything up.
#[test]
fn the_admin_runs_on_its_own_head() {
    let head = run_head(
        &admin_profile(),
        &channel("h1", HQ, &[ADMIN_VISION_ID]),
        &VisionsConfig::default(),
    );
    println!("{head}");
    assert!(head.starts_with("You are Atlas"));
    assert!(head.contains(r#"propose_visions_setup '{"channelId": "h1""#));
    assert!(head.contains(r#"apply_proposal '{"proposalId": "ID"}'"#));
    assert!(head.contains(r#"list_channels '{"projectRoot": "/work/app"}'"#));
    assert!(head.contains("one proposal per message"));
    assert!(head.contains("clear yes"));
    assert!(head.contains("Your shell runs only `themis`"));
    assert!(!head.contains("create_thread"), "it drives no chat");
    // Any other Vision keeps the ordinary head.
    let wren = vision("w", "Wren", None);
    let other = run_head(
        &wren,
        &channel("d", "docs", &["w"]),
        &VisionsConfig::default(),
    );
    assert!(other.starts_with("You are Wren, a Vision"));
}

/// Looking a proposal up reads `# hq` back from its end only so far ([`SCAN_BYTES`]),
/// however long the log: the work stays flat as the log doubles, and an older proposal is
/// unknown (proposed again).
#[test]
fn finding_a_proposal_reads_a_bounded_tail() {
    let samples = crate::scale::measure(&[150, 300], |size| {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new_for_test(dir.path().join("settings.json"));
        let root = dir.path().canonicalize().unwrap();
        state.visions_ensure_hq(&root).unwrap();
        let hq = state.channel_list(&root).remove(0).id;
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

/// Listing makes the admin once and each project's `# hq` once, and gives an `# hq` saved with
/// other members back to the admin alone.
#[test]
fn hq_is_made_once_per_project_with_the_admin_alone() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let root = dir.path().canonicalize().unwrap();
    state.visions_ensure_hq(&root).unwrap();
    state.visions_ensure_hq(&root).unwrap();
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
    // An `# hq` written by hand with another member.
    let mut cache = state.visions_cache();
    let channels = cache.channels(&state.inner.visions_dir).unwrap();
    let hq = channels.values_mut().next().unwrap();
    hq.members = vec![wren.id.clone()];
    drop(cache);
    state.visions_ensure_hq(&root).unwrap();
    assert_eq!(state.channel_list(&root)[0].members, [ADMIN_VISION_ID]);
}
