//! Visions: agents that drive ordinary Themis chats. This file holds the pure autonomy rules;
//! the `visions` configuration block is [`crate::configuration::VisionsConfig`].
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A kind of decision a Vision may meet while it drives a chat, in ladder order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Wording of text, names, messages.
    Wording,
    /// Which tests to write or run.
    Tests,
    /// A step that is easy to undo.
    Undoable,
    /// Trying a failed step again.
    Retry,
    /// Changing what the work covers.
    Scope,
    /// Deleting something.
    Delete,
    /// Adding or changing a dependency.
    Dependency,
    /// Anything that reaches outside the person's machine; always the person's.
    External,
    /// Anything that costs money; always the person's.
    Spend,
}

impl Decision {
    /// Every decision, in ladder order.
    pub const ALL: [Decision; 9] = [
        Decision::Wording,
        Decision::Tests,
        Decision::Undoable,
        Decision::Retry,
        Decision::Scope,
        Decision::Delete,
        Decision::Dependency,
        Decision::External,
        Decision::Spend,
    ];

    /// The id used in `visions.custom` and in logs.
    pub fn id(self) -> &'static str {
        match self {
            Decision::Wording => "wording",
            Decision::Tests => "tests",
            Decision::Undoable => "undoable",
            Decision::Retry => "retry",
            Decision::Scope => "scope",
            Decision::Delete => "delete",
            Decision::Dependency => "dependency",
            Decision::External => "external",
            Decision::Spend => "spend",
        }
    }

    /// The decision with this id.
    pub fn from_id(id: &str) -> Option<Decision> {
        Decision::ALL
            .into_iter()
            .find(|decision| decision.id() == id)
    }
}

/// How much a Vision decides on its own (`visions.autonomy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Autonomy {
    /// Everything goes to the person.
    Ask,
    /// Wording and tests.
    #[default]
    Small,
    /// Also undoable steps and retries.
    Most,
    /// Also scope, deletes and dependencies.
    NearlyAll,
    /// The rows in `visions.custom`.
    Custom,
}

impl Autonomy {
    /// Position on the preset ladder; `Custom` has none.
    pub fn rank(self) -> Option<u8> {
        match self {
            Autonomy::Ask => Some(0),
            Autonomy::Small => Some(1),
            Autonomy::Most => Some(2),
            Autonomy::NearlyAll => Some(3),
            Autonomy::Custom => None,
        }
    }
}

/// Whether the Vision decides this itself rather than asking the person. `External` and
/// `Spend` are always the person's, whatever the level or the custom rows say.
pub fn vision_decides(
    level: Autonomy,
    custom: &BTreeMap<String, bool>,
    decision: Decision,
) -> bool {
    if matches!(decision, Decision::External | Decision::Spend) {
        return false;
    }
    match level.rank() {
        Some(rank) => {
            let needed = match decision {
                Decision::Wording | Decision::Tests => 1,
                Decision::Undoable | Decision::Retry => 2,
                _ => 3,
            };
            rank >= needed
        }
        None => custom.get(decision.id()).copied().unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn presets_follow_the_ladder() {
        use Decision::*;
        let none = BTreeMap::new();
        // Decisions each level hands to the Vision, in ladder order.
        let table: [(Autonomy, &[Decision]); 4] = [
            (Autonomy::Ask, &[]),
            (Autonomy::Small, &[Wording, Tests]),
            (Autonomy::Most, &[Wording, Tests, Undoable, Retry]),
            (
                Autonomy::NearlyAll,
                &[Wording, Tests, Undoable, Retry, Scope, Delete, Dependency],
            ),
        ];
        for (level, decided) in table {
            for decision in Decision::ALL {
                assert_eq!(
                    vision_decides(level, &none, decision),
                    decided.contains(&decision),
                    "{level:?} / {}",
                    decision.id()
                );
            }
        }
    }

    #[test]
    fn external_and_spend_are_never_the_visions() {
        let custom = Decision::ALL
            .iter()
            .map(|d| (d.id().to_owned(), true))
            .collect::<BTreeMap<_, _>>();
        assert!(!vision_decides(
            Autonomy::Custom,
            &custom,
            Decision::External
        ));
        assert!(!vision_decides(Autonomy::Custom, &custom, Decision::Spend));
        assert!(vision_decides(Autonomy::Custom, &custom, Decision::Delete));
        assert!(!vision_decides(
            Autonomy::Custom,
            &BTreeMap::new(),
            Decision::Delete
        ));
    }

    #[test]
    fn ids_round_trip_in_order() {
        let ids = Decision::ALL.map(Decision::id);
        assert_eq!(
            ids,
            [
                "wording",
                "tests",
                "undoable",
                "retry",
                "scope",
                "delete",
                "dependency",
                "external",
                "spend"
            ]
        );
        for decision in Decision::ALL {
            assert_eq!(Decision::from_id(decision.id()), Some(decision));
        }
        assert_eq!(Decision::from_id("other"), None);
    }
}
