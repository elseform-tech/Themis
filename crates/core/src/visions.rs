//! Visions: agents that drive ordinary Themis chats. This file holds the pure level rules;
//! the `visions` configuration block is [`crate::configuration::VisionsConfig`].
use serde::{Deserialize, Serialize};

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

    /// The id used in a Vision's head and in logs.
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

/// A team's level (`visions.level`, a channel's `level`): how its chats run. Either way a chief
/// decides every [`Decision`] itself except the [`Decision::LOCKED`] ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// The team's chats run in Auto: a reviewer model judges what Custom would ask.
    #[default]
    AutoReview,
    /// The team's chats run in YOLO.
    FullAccess,
}

impl Decision {
    /// The decisions that always go to the person, at every level.
    pub const LOCKED: [Decision; 3] = [Decision::Delete, Decision::External, Decision::Spend];

    /// Whether this decision always goes to the person.
    pub fn is_locked(self) -> bool {
        Decision::LOCKED.contains(&self)
    }
}

/// The decisions a Vision makes itself, at every level: all but the locked ones, in ladder
/// order.
pub fn vision_decisions() -> Vec<Decision> {
    Decision::ALL
        .into_iter()
        .filter(|decision| !decision.is_locked())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vision_decides_all_but_delete_external_and_spend() {
        use Decision::*;
        assert_eq!(
            vision_decisions(),
            vec![Wording, Tests, Undoable, Retry, Scope, Dependency]
        );
        for locked in [Delete, External, Spend] {
            assert!(locked.is_locked(), "{}", locked.id());
        }
    }

    #[test]
    fn levels_are_written_in_snake_case_and_default_to_auto_review() {
        assert_eq!(Level::default(), Level::AutoReview);
        assert_eq!(
            serde_json::to_value(Level::FullAccess).unwrap(),
            "full_access"
        );
        assert_eq!(
            serde_json::from_value::<Level>("auto_review".into()).unwrap(),
            Level::AutoReview
        );
        assert!(serde_json::from_value::<Level>("small".into()).is_err());
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
