//! A person's message about plugins, skills, MCP servers or hooks brings the matching built-in
//! guide with its turn, the way picking `/mcp` (and the others) does, so the model never has to
//! decide to load it. The match is a plain word match on the person's own words
//! ([`themis_core::plugins::guides::integration_guides`]); automations, goal turns and hook wakes
//! get none.
use super::AppState;
use serde_json::json;
use themis_core::plugins::PluginStore;
use themis_core::skills::Skill;

impl AppState {
    /// The integration guides `text` calls for that it does not pick itself (`[[skill:mcp]]`),
    /// resolved like such a reference (with the run's folders); none for automations, goal turns
    /// and hook wakes. Logs which were attached (ids only, never the message).
    pub(super) fn integration_guides(
        &self,
        store: &PluginStore,
        legacy: &[Skill],
        text: &str,
        automation_run: bool,
        thread_id: &str,
    ) -> Vec<Skill> {
        let person_turn = !automation_run
            && themis_core::goal::turn_number(text).is_none()
            && !text.starts_with(themis_core::plugins::hooks::HOOK_WAKE_MARKER);
        if !person_turn {
            return Vec::new();
        }
        let wanted: Vec<&str> = themis_core::plugins::guides::integration_guides(text)
            .into_iter()
            .filter(|id| !text.contains(&format!("[[skill:{id}]]")))
            .collect();
        if wanted.is_empty() {
            return Vec::new();
        }
        let references: String = wanted.iter().map(|id| format!("[[skill:{id}]]")).collect();
        match store.resolve_prompt(&references, legacy) {
            Ok((_, guides, _)) => {
                let ids: Vec<&str> = guides.iter().map(|skill| skill.id.as_str()).collect();
                let _ = self.inner.diagnostics.append(
                    "skills",
                    "guides_attached",
                    "info",
                    json!({"thread_id": thread_id, "guides": ids}),
                );
                guides
            }
            Err(error) => {
                let _ = self.inner.diagnostics.append(
                    "skills",
                    "guides_unavailable",
                    "warn",
                    json!({"thread_id": thread_id, "guides": wanted, "error": error.to_string()}),
                );
                Vec::new()
            }
        }
    }
}

/// Adds each of `skills` to the turn's `resolved` skills unless one with its id is there.
pub(super) fn add_new(resolved: &mut Vec<Skill>, skills: impl IntoIterator<Item = Skill>) {
    for skill in skills {
        if !resolved.iter().any(|known| known.id == skill.id) {
            resolved.push(skill);
        }
    }
}
