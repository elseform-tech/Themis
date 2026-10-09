use super::*;
use themis_core::skills::validate_skill_input;

impl AppState {
    /// Lists all skills, ordered by id.
    pub async fn list_skills(&self) -> Vec<Skill> {
        let skills = self.inner.skills.read().await;
        let mut out: Vec<Skill> = skills.values().cloned().collect();
        out.sort_by(|left, right| left.id.cmp(&right.id));
        out
    }

    /// Creates a skill after validating the input with `themis-core`.
    pub async fn create_skill(&self, input: SkillInput) -> Result<Skill, String> {
        validate_skill_shapes(&input)?;
        let skill = Skill {
            id: uuid::Uuid::new_v4().to_string(),
            name: input.name,
            description: input.description,
            instructions: input.instructions,
            allowed_tools: input.allowed_tools,
            scripts: input.scripts,
        };
        self.inner
            .skills
            .write()
            .await
            .insert(skill.id.clone(), skill.clone());
        self.persist_skills().await;
        Ok(skill)
    }

    /// Replaces the skill `skill_id` with `input` (the id is kept).
    pub async fn update_skill(&self, skill_id: String, input: SkillInput) -> Result<Skill, String> {
        validate_skill_shapes(&input)?;
        let mut skills = self.inner.skills.write().await;
        let skill = skills
            .get_mut(&skill_id)
            .ok_or_else(|| format!("unknown skill '{skill_id}'"))?;
        skill.name = input.name;
        skill.description = input.description;
        skill.instructions = input.instructions;
        skill.allowed_tools = input.allowed_tools;
        skill.scripts = input.scripts;
        let updated = skill.clone();
        drop(skills);
        self.persist_skills().await;
        Ok(updated)
    }

    /// Deletes a skill and strips its id from every thread and automation.
    pub async fn delete_skill(&self, skill_id: String) -> Result<(), String> {
        {
            let mut skills = self.inner.skills.write().await;
            if skills.remove(&skill_id).is_none() {
                return Err(format!("unknown skill '{skill_id}'"));
            }
        }
        {
            let mut threads = self.inner.threads.write().await;
            for record in threads.values_mut() {
                record.skill_ids.retain(|id| id != &skill_id);
            }
        }
        {
            let mut automations = self.inner.automations.write().await;
            for automation in automations.values_mut() {
                automation.skill_ids.retain(|id| id != &skill_id);
            }
        }
        self.persist_skills().await;
        self.persist_registry().await;
        self.persist_automations().await;
        Ok(())
    }

    /// Attaches `skill_ids` to an idle thread (every id must exist).
    pub async fn set_thread_skills(
        &self,
        thread_id: String,
        skill_ids: Vec<String>,
    ) -> Result<ThreadInfo, String> {
        self.check_skill_ids(&skill_ids).await?;
        let info = {
            let mut threads = self.inner.threads.write().await;
            let record = threads
                .get_mut(&thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            if record.running {
                return Err(format!(
                    "thread '{thread_id}' is busy: wait for the run to finish \
                     before changing its skills"
                ));
            }
            record.skill_ids = dedup_ids(skill_ids);
            thread_info(record, &self.inner.transcript)?
        };
        self.persist_registry().await;
        Ok(info)
    }

    /// Rejects unknown skill ids, naming every bad one.
    pub(super) async fn check_skill_ids(&self, skill_ids: &[String]) -> Result<(), String> {
        let skills = self.inner.skills.read().await;
        let bad: Vec<&str> = skill_ids
            .iter()
            .map(String::as_str)
            .filter(|id| !skills.contains_key(*id))
            .collect();
        if bad.is_empty() {
            Ok(())
        } else {
            Err(format!("unknown skill id(s): {}", bad.join(", ")))
        }
    }
}

/// Validates skill input with `themis-core` and includes its detailed errors.
fn validate_skill_shapes(input: &SkillInput) -> Result<(), String> {
    let scripts: Vec<themis_core::skills::SkillScript> = input
        .scripts
        .iter()
        .map(|script| themis_core::skills::SkillScript {
            name: script.name.clone(),
            content: script.content.clone(),
        })
        .collect();
    validate_skill_input(
        &input.name,
        &input.instructions,
        &input.allowed_tools,
        &scripts,
    )
    .map_err(|err| err.to_string())
}
