//! Atlas's team and hire proposals: a whole team (a channel, its chief and workers, the team's
//! autonomy and budget) or one more worker for a channel. Checked field by field like any
//! proposal, and set up exactly as proposed after the person's reply. A team's autonomy and
//! budget are never above Settings › Visions. The crown is Atlas's alone.

use super::*;
use crate::state::visions::ACCESSORIES;

/// A whole team for a new channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewTeam {
    pub channel: TeamChannel,
    pub chief: NewMember,
    #[serde(default)]
    pub workers: Vec<NewMember>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<Autonomy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_budget_usd: Option<f64>,
}

/// The team's channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamChannel {
    pub name: String,
    #[serde(default)]
    pub topic: String,
}

/// A chief or a worker to make. Its look is chosen by the server when the proposal names none;
/// only a chief has a soul, and every worker has a role.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewMember {
    pub name: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub shape: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub accessory: String,
    #[serde(default)]
    pub soul: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// One more worker for an existing channel (by name), usually after its chief asked for one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hire {
    pub channel: String,
    pub worker: NewMember,
}

/// Whether a team's autonomy and budget stay within Settings › Visions.
pub(super) fn fits_settings(
    autonomy: Option<Autonomy>,
    budget: Option<f64>,
    config: &VisionsConfig,
) -> Result<(), String> {
    if autonomy == Some(Autonomy::Custom) {
        return Err(
            "Custom autonomy is set row by row in the app; pick a level instead".to_owned(),
        );
    }
    if let Some(level) = autonomy {
        let above = match (level.rank(), config.autonomy.rank()) {
            (Some(level), Some(allowed)) => level > allowed,
            // Custom on either side: only "ask", or exactly Settings' own level.
            _ => level != Autonomy::Ask && level != config.autonomy,
        };
        if above {
            return Err("A team's autonomy can't be above Settings › Visions".to_owned());
        }
    }
    if let Some(budget) = budget {
        if !(0.0..=config.daily_budget_usd).contains(&budget) {
            return Err(format!(
                "A team's daily budget is at most Settings › Visions' ${:.2}",
                config.daily_budget_usd
            ));
        }
    }
    Ok(())
}

/// A new member's profile.
pub(super) fn member_profile(new: &NewMember) -> Vision {
    Vision {
        id: String::new(),
        name: new.name.trim().to_owned(),
        role: new.role.trim().to_owned(),
        shape: new.shape.clone(),
        color: new.color.clone(),
        model: new.model.clone().filter(|model| !model.trim().is_empty()),
        accessory: if new.accessory.trim().is_empty() {
            "none".to_owned()
        } else {
            new.accessory.trim().to_owned()
        },
        // A chief's soul is saved once it leads (only a chief or Atlas may have one).
        soul: String::new(),
        paused: false,
        revision: String::new(),
    }
}

fn check_member(
    new: &NewMember,
    chief: bool,
    context: &Context,
    names: &mut HashSet<String>,
) -> Result<(), String> {
    let mut vision = member_profile(new);
    check_new_profile(&mut vision, context, names)?;
    if !ACCESSORIES.contains(&vision.accessory.as_str()) || vision.accessory == "crown" {
        return Err("Pick another accessory: the crown is Atlas's".to_owned());
    }
    if chief && new.soul.len() > crate::state::visions::SOUL_MAX_BYTES {
        return Err("A soul is at most 16 KiB".to_owned());
    }
    if !chief && !new.soul.trim().is_empty() {
        return Err("Only a chief of staff has a soul; a worker has a role".to_owned());
    }
    if !chief && vision.role.is_empty() {
        return Err(format!("Give {} a role", vision.name));
    }
    if let Some(model) = &vision.model {
        model_fits(&context.models, model)?;
    }
    Ok(())
}

/// Checks a team: a free channel name, a new chief and at least one new worker, and the team's
/// autonomy and budget within Settings.
pub(super) fn check_team(
    team: &NewTeam,
    context: &Context,
    names: &mut HashSet<String>,
    channel_names: &mut HashSet<String>,
) -> Result<(), String> {
    let mut channel = Channel {
        id: String::new(),
        name: team.channel.name.trim().to_owned(),
        topic: team.channel.topic.clone(),
        project_root: std::env::temp_dir(),
        full_access: false,
        members: Vec::new(),
        chief: String::new(),
        autonomy: team.autonomy,
        custom: None,
        daily_budget_usd: team.daily_budget_usd,
        paused_workers: Vec::new(),
        revision: String::new(),
    };
    validate_channel(&mut channel)?;
    let taken = context
        .channels
        .iter()
        .any(|other| other.name == channel.name);
    if taken || !channel_names.insert(channel.name.clone()) {
        return Err(format!(
            "This project already has a channel named {:?}",
            channel.name
        ));
    }
    check_member(&team.chief, true, context, names)?;
    if team.workers.is_empty() || team.workers.len() > MAX_ITEMS {
        return Err("A team has 1 to 16 workers".to_owned());
    }
    for worker in &team.workers {
        check_member(worker, false, context, names)?;
    }
    fits_settings(team.autonomy, team.daily_budget_usd, &context.config)
}

/// Checks a hire: an existing channel of the project (not `# administration`) and a new worker.
pub(super) fn check_hire(
    hire: &Hire,
    context: &Context,
    names: &mut HashSet<String>,
) -> Result<(), String> {
    if hire.channel.trim() == ADMINISTRATION {
        return Err("# administration is Atlas's; hire workers for another channel".to_owned());
    }
    context.channel(&hire.channel)?;
    check_member(&hire.worker, false, context, names)
}

/// Gives each new member without a look one, in turn after `existing` Visions.
pub(super) fn fill_member_looks(proposal: &mut Proposal, existing: usize) {
    let mut members: Vec<&mut NewMember> = Vec::new();
    if let Some(team) = proposal.team.as_mut() {
        members.push(&mut team.chief);
        members.extend(team.workers.iter_mut());
    }
    if let Some(hire) = proposal.hire.as_mut() {
        members.push(&mut hire.worker);
    }
    for (at, member) in members.into_iter().enumerate() {
        if member.shape.trim().is_empty() {
            member.shape = SHAPES[(existing + at) % SHAPES.len()].to_owned();
        }
        if member.color.trim().is_empty() {
            member.color = COLORS[(existing + at) % COLORS.len()].to_owned();
        }
    }
}

/// What one team or hire set-up made so far, to undo when a later step fails.
#[derive(Default)]
struct Made {
    items: Vec<SetUp>,
    visions: Vec<String>,
    channel: Option<String>,
}

impl Made {
    fn vision(&mut self, vision: &Vision) {
        self.visions.push(vision.id.clone());
        self.items.push(set_up(
            SetUpKind::Vision,
            &vision.id,
            &vision.name,
            SetUpAction::Created,
        ));
    }
}

impl AppState {
    /// Makes the proposal's team and hire, noting each in `items` as it is done. Each is all or
    /// nothing: when a step fails, what that team or hire made is removed again, so no stray
    /// chief or worker is left behind.
    pub(super) fn visions_set_up_team(
        &self,
        hq: &Channel,
        proposal: &Proposal,
        items: &mut Vec<SetUp>,
    ) -> Result<(), String> {
        if let Some(team) = &proposal.team {
            let mut made = Made::default();
            let built = self.build_team(hq, team, &mut made);
            self.keep_or_undo(built, made, items)?;
        }
        if let Some(hire) = &proposal.hire {
            let mut made = Made::default();
            let built = self.build_hire(hq, hire, &mut made);
            self.keep_or_undo(built, made, items)?;
        }
        Ok(())
    }

    fn keep_or_undo(
        &self,
        built: Result<(), String>,
        made: Made,
        items: &mut Vec<SetUp>,
    ) -> Result<(), String> {
        if built.is_ok() {
            items.extend(made.items);
            return built;
        }
        let dir = &self.inner.visions_dir;
        let mut cache = self.visions_cache();
        if let Some(channel) = &made.channel {
            let _ = cache.delete_channel(dir, channel);
        }
        for vision in &made.visions {
            let _ = cache.delete_vision(dir, vision);
        }
        drop(cache);
        self.visions_note(
            "proposal_undone",
            json!({"visions": made.visions.len(), "channel": made.channel.is_some()}),
        );
        built
    }

    fn build_team(&self, hq: &Channel, team: &NewTeam, made: &mut Made) -> Result<(), String> {
        let chief = self.vision_save(member_profile(&team.chief), None)?;
        made.vision(&chief);
        let mut members = vec![chief.id.clone()];
        for worker in &team.workers {
            let saved = self.vision_save(member_profile(worker), None)?;
            made.vision(&saved);
            members.push(saved.id);
        }
        let spec = Channel {
            id: String::new(),
            name: team.channel.name.trim().to_owned(),
            topic: team.channel.topic.clone(),
            project_root: hq.project_root.clone(),
            full_access: false,
            members,
            chief: chief.id.clone(),
            autonomy: team.autonomy,
            custom: None,
            daily_budget_usd: team.daily_budget_usd,
            paused_workers: Vec::new(),
            revision: String::new(),
        };
        let channel = self.channel_save(spec, None)?;
        made.channel = Some(channel.id.clone());
        made.items.push(set_up(
            SetUpKind::Channel,
            &channel.id,
            &channel.name,
            SetUpAction::Created,
        ));
        let soul = team.chief.soul.trim();
        if !soul.is_empty() {
            let revision = chief.revision.clone();
            let with_soul = Vision {
                soul: soul.to_owned(),
                ..chief
            };
            self.vision_save(with_soul, Some(&revision))?;
        }
        Ok(())
    }

    fn build_hire(&self, hq: &Channel, hire: &Hire, made: &mut Made) -> Result<(), String> {
        let channel = self
            .channel_list(&hq.project_root)
            .into_iter()
            .find(|channel| channel.name == hire.channel.trim())
            .ok_or_else(|| format!("This project has no channel named {:?}", hire.channel))?;
        let worker = self.vision_save(member_profile(&hire.worker), None)?;
        made.vision(&worker);
        let revision = channel.revision.clone();
        let mut joined = channel;
        joined.members.push(worker.id);
        let saved = self.channel_save(joined, Some(&revision))?;
        made.items.push(set_up(
            SetUpKind::Channel,
            &saved.id,
            &saved.name,
            SetUpAction::Changed,
        ));
        Ok(())
    }
}

#[cfg(test)]
#[path = "admin_teams_tests.rs"]
mod tests;
