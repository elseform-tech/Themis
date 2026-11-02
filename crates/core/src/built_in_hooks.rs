//! Themis's own hooks (`hooks.built_in`, Integrations › Hooks as "Themis (built-in)"). They are
//! native: no command, no approval, and they never grant permissions; each only adds a short
//! piece of text for the model. All are off by default.

/// The short To-do skill a run carries while `skills_at_start` is on.
pub const TODO_SKILL: &str = include_str!("../builtins/todo/SKILL.md");
/// The short Memory skill a run carries while `skills_at_start` is on and memory is on.
pub const MEMORY_SKILL: &str = include_str!("../builtins/memory/SKILL.md");
/// Most bytes each of those skills may have.
pub const MAX_SKILL_BYTES: usize = 1536;

/// One built-in hook as Integrations › Hooks lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct BuiltInHook {
    /// Its switch in `hooks.built_in`.
    pub id: &'static str,
    /// The hook event it belongs to, as plugin hooks name them.
    pub event: &'static str,
    pub title: &'static str,
    pub description: &'static str,
}

/// Every built-in hook, in run order.
pub const ALL: [BuiltInHook; 3] = [
    BuiltInHook {
        id: "skills_at_start",
        event: "SessionStart",
        title: "To-do and Memory skills",
        description: "Each turn carries short instructions for keeping a to-do list and saving memories",
    },
    BuiltInHook {
        id: "save_reminder",
        event: "BeforeCompaction",
        title: "Save before compaction",
        description: "When the context is about to be compacted, the agent is reminded to save anything worth remembering",
    },
    BuiltInHook {
        id: "still_open",
        event: "RunEnd",
        title: "Still open",
        description: "When files changed while to-do items are open, the next turn starts by naming them",
    },
];

/// A skill file's text without its `---` header.
fn body(skill: &str) -> &str {
    skill
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map_or(skill, |(_, body)| body)
        .trim()
}

/// What `skills_at_start` adds to a run: the To-do skill, and the Memory skill while memory is
/// on.
pub fn skills_text(memory_enabled: bool) -> String {
    let mut text = body(TODO_SKILL).to_owned();
    if memory_enabled {
        text.push_str("\n\n");
        text.push_str(body(MEMORY_SKILL));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skills_are_short_concrete_and_headed() {
        for skill in [TODO_SKILL, MEMORY_SKILL] {
            assert!(skill.len() <= MAX_SKILL_BYTES, "{} bytes", skill.len());
            assert!(skill.starts_with("---\nname: "), "{skill}");
            assert!(skill.contains("```json") || skill.contains("{\"action\""));
        }
        let both = skills_text(true);
        assert!(both.starts_with("# To-do list"), "{both}");
        assert!(both.contains("\n\n# Memory\n"), "{both}");
        assert!(!both.contains("name: todo"));
        assert!(!skills_text(false).contains("# Memory"));
    }

    #[test]
    fn every_switch_is_listed_once() {
        let switches = crate::configuration::BuiltInHooks::default().switches();
        assert_eq!(switches.len(), ALL.len());
        for (name, _) in switches {
            assert_eq!(
                ALL.iter().filter(|hook| hook.id == name).count(),
                1,
                "{name}"
            );
        }
    }
}
