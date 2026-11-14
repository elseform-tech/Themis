//! Atlas's team and hire proposals, checked field by field against what exists and against
//! Settings › Visions (a team's autonomy and budget are never above them).

use serde_json::json;

use super::super::tests::{context, proposal};
use super::*;

fn team(extra: serde_json::Value) -> serde_json::Value {
    let mut team = json!({
        "channel": {"name": "release", "topic": "Ship 0.9"},
        "chief": {"name": "Juno", "soul": "Calm and direct.", "accessory": "glasses"},
        "workers": [{"name": "Rook", "role": "Builds"}, {"name": "Tess", "role": "Tests"}]
    });
    for (key, value) in extra.as_object().unwrap() {
        team[key] = value.clone();
    }
    json!({"summary": "A release team.", "team": team})
}

#[test]
fn a_team_proposal_is_checked_field_by_field() {
    assert_eq!(check(&proposal(team(json!({}))), &context()), Ok(()));
    let bad = [
        (
            json!({"channel": {"name": "docs"}}),
            "already has a channel",
        ),
        (json!({"chief": {"name": "Wren"}}), "already exists"),
        (json!({"workers": []}), "worker"),
        (
            json!({"workers": [{"name": "Juno", "role": "x"}]}),
            "already exists",
        ),
        (json!({"workers": [{"name": "Rook"}]}), "role"),
        (json!({"chief": {"name": "Juno", "model": "nope"}}), "model"),
        (
            json!({"chief": {"name": "Juno", "accessory": "crown"}}),
            "accessory",
        ),
    ];
    for (extra, words) in bad {
        let error = check(&proposal(team(extra.clone())), &context()).unwrap_err();
        assert!(error.contains(words), "{extra}: {error}");
    }
}

#[test]
fn a_proposal_cannot_raise_autonomy_or_budget() {
    // Settings › Visions: "small" calls and $1 a day (the defaults).
    let fits = [json!({"autonomy": "ask"}), json!({"daily_budget_usd": 0.5})];
    for extra in fits {
        assert_eq!(
            check(&proposal(team(extra.clone())), &context()),
            Ok(()),
            "{extra}"
        );
    }
    let above = [
        json!({"autonomy": "most"}),
        json!({"daily_budget_usd": 2.0}),
    ];
    for extra in above {
        let error = check(&proposal(team(extra.clone())), &context()).unwrap_err();
        assert!(error.contains("Settings"), "{extra}: {error}");
    }
    // The same for a change to an existing channel.
    let raise = json!({"summary": "x", "changes": [{"channel": "docs", "autonomy": "nearly_all"}]});
    assert!(check(&proposal(raise), &context())
        .unwrap_err()
        .contains("Settings"));
    let lower = json!({"summary": "x", "changes": [{"channel": "docs", "daily_budget_usd": 0.2}]});
    assert_eq!(check(&proposal(lower), &context()), Ok(()));
}

#[test]
fn a_hire_is_checked() {
    let hire = |channel: &str, name: &str| {
        proposal(
            json!({"summary": "A tester.", "hire": {"channel": channel, "worker": {"name": name, "role": "Tests"}}}),
        )
    };
    assert_eq!(check(&hire("docs", "Tess"), &context()), Ok(()));
    assert!(check(&hire("administration", "Tess"), &context())
        .unwrap_err()
        .contains("administration"));
    assert!(check(&hire("nowhere", "Tess"), &context())
        .unwrap_err()
        .contains("no channel"));
    assert!(check(&hire("docs", "Wren"), &context())
        .unwrap_err()
        .contains("already exists"));
}

/// A team whose channel cannot be saved leaves nothing behind: no stray chief or workers.
#[test]
fn a_team_that_cannot_be_set_up_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("p");
    std::fs::create_dir_all(&root).unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    state.visions_ensure_administration(&root).unwrap();
    let hq = state
        .channel_list(&root)
        .into_iter()
        .find(|channel| channel.name == ADMINISTRATION)
        .unwrap();
    // A channel named "release" already exists, so the team's channel save fails.
    let mut taken = hq.clone();
    taken.id = String::new();
    taken.name = "release".into();
    taken.members = Vec::new();
    taken.chief = String::new();
    state.channel_save(taken, None).unwrap();
    let before = state.visions_list().len();
    let team = proposal(team(json!({})));
    let mut items = Vec::new();
    assert!(state.visions_set_up_team(&hq, &team, &mut items).is_err());
    assert_eq!(state.visions_list().len(), before, "no stray Visions");
    assert!(items.is_empty(), "{items:?}");
}

#[test]
fn a_team_cannot_ask_for_custom_rows() {
    let error = check(&proposal(team(json!({"autonomy": "custom"}))), &context()).unwrap_err();
    assert!(error.contains("Custom"), "{error}");
}

/// A team or a hire is the whole proposal: the card shows only it, so nothing else may ride along
/// unseen on the person's yes.
#[test]
fn a_team_or_hire_comes_alone() {
    let mut with_vision = team(json!({}));
    with_vision["visions"] = json!([{"name": "Mira", "role": "Tests"}]);
    let mut with_hire = team(json!({}));
    with_hire["hire"] = json!({"channel": "docs", "worker": {"name": "Ivo", "role": "Docs"}});
    let hire_with_change = json!({"summary": "A tester.",
        "hire": {"channel": "docs", "worker": {"name": "Ivo", "role": "Tests"}},
        "changes": [{"channel": "docs", "topic": "Docs"}]});
    for mixed in [with_vision, with_hire, hire_with_change] {
        let error = check(&proposal(mixed), &context()).unwrap_err();
        assert!(error.contains("on its own"), "{error}");
    }
}
