//! Skill bundles and their create/update payload.

use serde::{Deserialize, Serialize};

/// A helper script bundled with a skill (`SkillScript` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillScript {
    pub name: String,
    pub content: String,
}

/// A skill bundle (`Skill` in types.ts).
///
/// The wire shape is `snake_case` like the rest of the bridge; use
/// [`Skill::core_skill`] to convert to the `themis-core` representation
/// (which serializes `camelCase`) before calling core skill APIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub instructions: String,
    /// Tool-name allowlist. Empty means all tools.
    pub allowed_tools: Vec<String>,
    pub scripts: Vec<SkillScript>,
}

impl Skill {
    /// Converts to the `themis-core` skill representation.
    #[must_use]
    pub fn core_skill(&self) -> themis_core::skills::Skill {
        themis_core::skills::Skill {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            instructions: self.instructions.clone(),
            allowed_tools: self.allowed_tools.clone(),
            scripts: self
                .scripts
                .iter()
                .map(|script| themis_core::skills::SkillScript {
                    name: script.name.clone(),
                    content: script.content.clone(),
                })
                .collect(),
        }
    }
}

/// Skill create/update payload (`SkillInput` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillInput {
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub allowed_tools: Vec<String>,
    pub scripts: Vec<SkillScript>,
}
