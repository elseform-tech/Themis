//! Atlas's team and hire proposals, checked field by field against what exists and against
//! Settings › Visions (a team's budget is never above them; a proposal sets no level).

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
fn a_proposal_cannot_raise_the_budget_or_set_a_level() {
    // Settings › Visions: $1 a day (the default).
    let fits = json!({"daily_budget_usd": 0.5});
    assert_eq!(check(&proposal(team(fits)), &context()), Ok(()));
    let above = json!({"daily_budget_usd": 2.0});
    let error = check(&proposal(team(above)), &context()).unwrap_err();
    assert!(error.contains("Settings"), "{error}");
    // A level is the person's: a team naming one does not read.
    for level in [json!({"level": "full_access"}), json!({"autonomy": "ask"})] {
        assert!(
            serde_json::from_value::<Proposal>(team(level.clone())).is_err(),
            "{level}"
        );
    }
    // The same budget rule for a change to an existing channel.
    let raise = json!({"summary": "x", "changes": [{"channel": "docs", "daily_budget_usd": 2.0}]});
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
