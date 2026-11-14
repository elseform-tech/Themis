//! The built-in admin Vision, Atlas, in each project's `# administration`.
//!
//! One per install (id [`ADMIN_VISION_ID`]), saved the first time a project's channels are
//! listed, which also makes that project's `# administration` with the admin as its only member. Neither can
//! be deleted, the admin works in no other channel, and `# administration` has no other member (`store`).
//! The admin builds nothing itself: it posts a proposal (channels, Visions and changes to them)
//! as a `proposal` entry, and sets it up with `apply_proposal` only when the proposal was never
//! set up and the person wrote in `# administration` after it. Whether that reply means yes is the model's
//! judgement; the server checks everything else, each field again when it is set up: a proposal
//! never raises autonomy above Settings › Visions, never raises a budget, never deletes, never
//! touches the admin or `# administration`, and its channels start in Custom. Logged as metadata only
//! (`proposal_posted`, `proposal_refused`, `proposal_applied`: ids, counts and outcome).

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::json;
use themis_core::configuration::{ApprovalMode, VisionsConfig};
use themis_core::prompts::{fill, VISIONS_ADMIN};
use themis_core::visions::Autonomy;

use super::store::{validate_channel, validate_vision};
use super::{Channel, ChannelEntry, Vision};
use crate::state::AppState;

/// The admin Vision's id.
pub const ADMIN_VISION_ID: &str = "admin";
/// The name of each project's built-in channel.
pub(super) const ADMINISTRATION: &str = "administration";
/// Why a proposal may not set autonomy or a budget on a Vision: they are the channel's.
const TEAM_SETTINGS: &str =
    "A proposal cannot set autonomy or a budget on a Vision: they belong to its channel now";
/// `# administration`'s name before teams (see `convert`).
pub(super) const OLD_HQ: &str = "hq";
/// Why the admin is never deleted.
pub(super) const ADMIN_STAYS: &str = "Atlas is built in and can't be deleted";
/// Why `# administration` is never deleted.
pub(super) const ADMINISTRATION_STAYS: &str = "# administration is built in and can't be deleted";
const MAX_SUMMARY_CHARS: usize = 600;
/// Most channels, Visions or changes one proposal holds, each.
const MAX_ITEMS: usize = 16;
/// How far back in `# administration` a proposal is looked for; an older one is proposed again.
const SCAN_BYTES: usize = 512 * 1024;
const PAGE_BYTES: usize = 64 * 1024;
/// The looks a new Vision may have (the app's cast).
const SHAPES: [&str; 7] = [
    "tall", "flat", "two-bump", "leaning", "wide", "fluffy", "broad",
];
const COLORS: [&str; 8] = [
    "blue", "teal", "violet", "amber", "rose", "indigo", "slate", "coral",
];

/// The admin's profile as it is first saved.
pub(super) fn admin_profile() -> Vision {
    Vision {
        id: ADMIN_VISION_ID.to_owned(),
        name: "Atlas".to_owned(),
        role: "Sets up channels and Visions".to_owned(),
        shape: "flat".to_owned(),
        color: "indigo".to_owned(),
        model: None,
        accessory: "crown".to_owned(),
        paused: false,
        revision: String::new(),
    }
}

pub fn is_admin(vision_id: &str) -> bool {
    vision_id == ADMIN_VISION_ID
}

/// What the admin would set up, as it posts it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    /// What it would set up, in the admin's plain words.
    pub summary: String,
    #[serde(default)]
    pub channels: Vec<NewChannel>,
    #[serde(default)]
    pub visions: Vec<NewVision>,
    #[serde(default)]
    pub changes: Vec<Change>,
}

/// A channel to make in `# administration`'s project; `members` are Vision names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewChannel {
    pub name: String,
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub members: Vec<String>,
}

/// A Vision to make. Its look is chosen by the server when the proposal names none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewVision {
    pub name: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub shape: String,
    #[serde(default)]
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<Autonomy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_budget_usd: Option<f64>,
}

/// A change to one existing Vision (`vision`, by name) or channel (`channel`, by name).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<Autonomy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_budget_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_members: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_members: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetUpKind {
    Vision,
    Channel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetUpAction {
    Created,
    Changed,
}

/// One thing a proposal set up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetUp {
    pub kind: SetUpKind,
    pub id: String,
    pub name: String,
    pub action: SetUpAction,
}

/// What a proposal is checked against: every Vision, `# administration`'s project's channels and the
/// project's resolved `visions` block.
pub(super) struct Context {
    pub visions: Vec<Vision>,
    pub channels: Vec<Channel>,
    #[allow(dead_code)] // Team proposals check autonomy and budget against it (plan Task 6).
    pub config: VisionsConfig,
    /// The usable models of the loaded catalog (empty while none is loaded).
    pub models: Vec<String>,
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

impl Context {
    /// The Vision named `name`; an error when none or several have that name.
    fn vision(&self, name: &str) -> Result<&Vision, String> {
        let mut found = self.visions.iter().filter(|v| same_name(&v.name, name));
        match (found.next(), found.next()) {
            (Some(vision), None) => Ok(vision),
            (None, _) => Err(format!("No Vision is named {:?}", name.trim())),
            (Some(_), Some(_)) => Err(format!("Several Visions are named {:?}", name.trim())),
        }
    }

    fn channel(&self, name: &str) -> Result<&Channel, String> {
        self.channels
            .iter()
            .find(|channel| channel.name == name.trim())
            .ok_or_else(|| format!("This project has no channel named {:?}", name.trim()))
    }
}

/// Whether `model` is a usable model of the loaded catalog.
fn model_fits(models: &[String], model: &str) -> Result<(), String> {
    if models.iter().any(|known| known == model.trim()) {
        return Ok(());
    }
    if models.is_empty() {
        return Err("Themis cannot check models right now; leave the model out".to_owned());
    }
    Err(format!("No usable model is named {:?}", model.trim()))
}

/// Checks a proposal against what exists now, field by field.
pub(super) fn check(proposal: &Proposal, context: &Context) -> Result<(), String> {
    let summary = proposal.summary.trim();
    if summary.is_empty() || summary.chars().count() > MAX_SUMMARY_CHARS {
        return Err("A proposal needs a summary of at most 600 characters".to_owned());
    }
    let counts = [
        proposal.channels.len(),
        proposal.visions.len(),
        proposal.changes.len(),
    ];
    if counts.iter().all(|count| *count == 0) || counts.iter().any(|count| *count > MAX_ITEMS) {
        return Err("A proposal holds 1 to 16 channels, Visions and changes, each".to_owned());
    }
    let mut names = HashSet::new();
    for vision in &proposal.visions {
        check_new_vision(vision, context, &mut names)?;
    }
    let mut channel_names = HashSet::new();
    for channel in &proposal.channels {
        check_new_channel(channel, context, &names, &mut channel_names)?;
    }
    for change in &proposal.changes {
        check_change(change, context, &names)?;
    }
    Ok(())
}

fn check_new_vision(
    new: &NewVision,
    context: &Context,
    names: &mut HashSet<String>,
) -> Result<(), String> {
    let mut vision = new_profile(new);
    validate_vision(&mut vision)?;
    let taken = context
        .visions
        .iter()
        .any(|other| same_name(&other.name, &vision.name));
    if taken || !names.insert(vision.name.to_lowercase()) {
        return Err(format!("A Vision named {:?} already exists", vision.name));
    }
    if !SHAPES.contains(&new.shape.as_str()) || !COLORS.contains(&new.color.as_str()) {
        return Err(format!(
            "A Vision's shape is one of {SHAPES:?} and its colour one of {COLORS:?}"
        ));
    }
    if new.autonomy.is_some() || new.daily_budget_usd.is_some() {
        return Err(TEAM_SETTINGS.to_owned());
    }
    if let Some(model) = new
        .model
        .as_deref()
        .filter(|model| !model.trim().is_empty())
    {
        model_fits(&context.models, model)?;
    }
    Ok(())
}

/// The members a channel would have: new Visions by name (`new`), or saved ones; an id, or the
/// lowercased name for a Vision the proposal makes.
fn member(context: &Context, new: &HashSet<String>, name: &str) -> Result<String, String> {
    if new.contains(&name.trim().to_lowercase()) {
        return Ok(name.trim().to_lowercase());
    }
    let vision = context.vision(name)?;
    if is_admin(&vision.id) {
        return Err("Atlas works only in # administration".to_owned());
    }
    Ok(vision.id.clone())
}

fn check_new_channel(
    new: &NewChannel,
    context: &Context,
    visions: &HashSet<String>,
    names: &mut HashSet<String>,
) -> Result<(), String> {
    let members = new
        .members
        .iter()
        .map(|name| member(context, visions, name))
        .collect::<Result<Vec<_>, _>>()?;
    let mut channel = Channel {
        id: String::new(),
        name: new.name.clone(),
        topic: new.topic.clone(),
        project_root: std::env::temp_dir(),
        approval_mode: ApprovalMode::Custom,
        members,
        chief: String::new(),
        autonomy: None,
        custom: None,
        daily_budget_usd: None,
        revision: String::new(),
    };
    validate_channel(&mut channel)?;
    let taken = context
        .channels
        .iter()
        .any(|other| other.name == channel.name);
    if taken || !names.insert(channel.name.clone()) {
        return Err(format!(
            "This project already has a channel named {:?}",
            channel.name
        ));
    }
    Ok(())
}

fn check_change(change: &Change, context: &Context, new: &HashSet<String>) -> Result<(), String> {
    match (&change.vision, &change.channel) {
        (Some(name), None) => check_vision_change(change, context.vision(name)?, context),
        (None, Some(name)) => check_channel_change(change, context.channel(name)?, context, new),
        _ => Err("A change names either one Vision or one channel".to_owned()),
    }
}

fn check_vision_change(change: &Change, vision: &Vision, context: &Context) -> Result<(), String> {
    if is_admin(&vision.id) {
        return Err("Atlas is built in; a proposal cannot change it".to_owned());
    }
    if change.autonomy.is_some() || change.daily_budget_usd.is_some() {
        return Err(TEAM_SETTINGS.to_owned());
    }
    if change.topic.is_some() || !change.add_members.is_empty() || !change.remove_members.is_empty()
    {
        return Err("A Vision's change may set its name, role or paused".to_owned());
    }
    let changed = applied_to_vision(change, vision);
    if changed == *vision {
        return Err(format!("The change to {} changes nothing", vision.name));
    }
    validate_vision(&mut changed.clone())?;
    if let Some(name) = &change.name {
        let taken = context
            .visions
            .iter()
            .any(|other| other.id != vision.id && same_name(&other.name, name));
        if taken {
            return Err(format!("A Vision named {:?} already exists", name.trim()));
        }
    }
    Ok(())
}

fn check_channel_change(
    change: &Change,
    channel: &Channel,
    context: &Context,
    new: &HashSet<String>,
) -> Result<(), String> {
    if channel.name == ADMINISTRATION {
        return Err("# administration is built in; a proposal cannot change it".to_owned());
    }
    let vision_only = change.role.is_some()
        || change.paused.is_some()
        || change.autonomy.is_some()
        || change.daily_budget_usd.is_some();
    if vision_only {
        return Err("A channel's change may set its name, topic and members".to_owned());
    }
    let members = |names: &[String]| {
        names
            .iter()
            .map(|name| member(context, new, name))
            .collect::<Result<Vec<_>, _>>()
    };
    let mut changed = applied_to_channel(
        change,
        channel,
        &members(&change.add_members)?,
        &members(&change.remove_members)?,
    );
    if changed == *channel {
        return Err(format!("The change to #{} changes nothing", channel.name));
    }
    validate_channel(&mut changed)?;
    let taken = context
        .channels
        .iter()
        .any(|other| other.id != channel.id && other.name == changed.name);
    if taken {
        return Err(format!(
            "This project already has a channel named {:?}",
            changed.name
        ));
    }
    Ok(())
}

/// A new Vision's profile.
fn new_profile(new: &NewVision) -> Vision {
    Vision {
        id: String::new(),
        name: new.name.clone(),
        role: new.role.clone(),
        shape: new.shape.clone(),
        color: new.color.clone(),
        model: new.model.clone(),
        accessory: "none".to_owned(),
        paused: false,
        revision: String::new(),
    }
}

/// `vision` with the change's name, role, autonomy and budget (its pause is set apart).
fn applied_to_vision(change: &Change, vision: &Vision) -> Vision {
    let mut changed = vision.clone();
    if let Some(name) = &change.name {
        changed.name = name.trim().to_owned();
    }
    if let Some(role) = &change.role {
        changed.role = role.trim().to_owned();
    }
    changed.paused = change.paused.unwrap_or(changed.paused);
    changed
}

/// `channel` with the change's name and topic, `add` members added and `remove` taken out.
fn applied_to_channel(
    change: &Change,
    channel: &Channel,
    add: &[String],
    remove: &[String],
) -> Channel {
    let mut changed = channel.clone();
    if let Some(name) = &change.name {
        changed.name = name.trim().to_owned();
    }
    if let Some(topic) = &change.topic {
        changed.topic = topic.trim().to_owned();
    }
    changed.members.retain(|member| !remove.contains(member));
    for member in add {
        if !changed.members.contains(member) {
            changed.members.push(member.clone());
        }
    }
    // The chief leaves with its membership: the first member left leads.
    if !changed.members.contains(&changed.chief) {
        changed.chief = changed.members.first().cloned().unwrap_or_default();
    }
    changed
}

/// Gives each new Vision without a look one, in turn after the Visions there are.
fn fill_looks(proposal: &mut Proposal, existing: usize) {
    for (at, vision) in proposal.visions.iter_mut().enumerate() {
        if vision.shape.trim().is_empty() {
            vision.shape = SHAPES[(existing + at) % SHAPES.len()].to_owned();
        }
        if vision.color.trim().is_empty() {
            vision.color = COLORS[(existing + at) % COLORS.len()].to_owned();
        }
    }
}

/// The admin run's system head; any other Vision's is [`super::runs::vision_head`].
pub(super) fn run_head(
    vision: &Vision,
    channel: &Channel,
    config: &VisionsConfig,
    soul: &str,
) -> String {
    if !is_admin(&vision.id) {
        return super::runs::vision_head(vision, channel, config, soul);
    }
    let project =
        serde_json::to_string(&channel.project_root.to_string_lossy()).unwrap_or_default();
    fill(
        VISIONS_ADMIN,
        &[
            ("name", &vision.name),
            ("channel_id", &channel.id),
            ("project", project.trim_matches('"')),
        ],
    )
}

/// A proposal as found in `# administration`: what it holds, and what came after it.
struct Found {
    proposal: Proposal,
    /// The person wrote in `# administration` after it.
    replied: bool,
    /// It was set up already.
    applied: bool,
    /// The admin posted a newer proposal in `# administration` after it.
    superseded: bool,
}

/// Why setting up a proposal was refused, as the log keeps it, and what the admin is told.
struct Refused(&'static str, String);

impl AppState {
    /// Makes the admin and the `# administration` of the project at `root` (canonical) when they are not
    /// there, and gives an `# administration` with other members back to the admin alone.
    pub(super) fn visions_ensure_administration(&self, root: &Path) -> Result<(), String> {
        let created = self.visions_cache().ensure_admin(&self.inner.visions_dir);
        if created.map_err(|error| self.visions_failed(error))? {
            self.visions_note("admin_created", json!({"vision_id": ADMIN_VISION_ID}));
            self.visions_announce_all();
        }
        let told = self
            .visions_cache()
            .finish_conversion(&self.inner.visions_dir)
            .map_err(|error| self.visions_failed(error))?;
        if !told.is_empty() {
            self.visions_note("teams_arrived", json!({"channels": told.len()}));
            self.visions_announce_all();
        }
        let existing = self
            .channel_list(root)
            .into_iter()
            .find(|channel| channel.name == ADMINISTRATION);
        let admin_only = vec![ADMIN_VISION_ID.to_owned()];
        let (spec, expected) = match existing {
            Some(hq) if hq.members == admin_only => return Ok(()),
            Some(hq) => {
                let revision = hq.revision.clone();
                (
                    Channel {
                        members: admin_only,
                        chief: ADMIN_VISION_ID.to_owned(),
                        ..hq
                    },
                    Some(revision),
                )
            }
            None => (
                Channel {
                    id: String::new(),
                    name: ADMINISTRATION.to_owned(),
                    topic: String::new(),
                    project_root: root.to_path_buf(),
                    approval_mode: ApprovalMode::Custom,
                    members: admin_only,
                    chief: ADMIN_VISION_ID.to_owned(),
                    autonomy: None,
                    custom: None,
                    daily_budget_usd: None,
                    revision: String::new(),
                },
                None,
            ),
        };
        let event = if expected.is_some() {
            "hq_members_reset"
        } else {
            "hq_created"
        };
        let saved = self.channel_save(spec, expected.as_deref())?;
        self.visions_note(event, json!({"channel_id": saved.id}));
        self.visions_announce_all();
        Ok(())
    }

    /// Tells the app to read every Visions list again: Atlas or a `# administration` was made while it
    /// read the channels, perhaps after it read the Visions.
    fn visions_announce_all(&self) {
        if let Some(sink) = self.scheduler_sink() {
            sink.emit_visions_changed(&crate::types::VisionsChange::default());
        }
    }

    /// The models a proposal may name, as the app resolves them: the usable OpenCode Go
    /// models (the catalog loaded first when it is not yet, within a few seconds) and the
    /// models of the providers your `runtime.jsonc` defines, by their qualified
    /// `<provider>/<model>` id only. Without a Go key the Go part is empty.
    pub(crate) async fn visions_usable_models(&self) -> Vec<String> {
        let wait = std::time::Duration::from_secs(3);
        let _ = tokio::time::timeout(wait, self.ensure_go_catalog()).await;
        let mut models: Vec<String> = self
            .inner
            .go_catalog
            .read()
            .await
            .iter()
            .filter(|model| model.unavailable_reason.is_none())
            .map(|model| model.id.clone())
            .collect();
        // Only the qualified id routes a chat to its provider; a bare one would run on Go.
        for provider in self.user_providers() {
            let listed = self.provider_model_list(&provider).await;
            models.extend(listed.into_iter().map(|model| model.id));
        }
        models
    }

    fn visions_admin_context(
        &self,
        channel_id: &str,
        models: Vec<String>,
    ) -> Result<(Channel, Context), String> {
        let hq = self
            .channel_get(channel_id)
            .filter(|channel| channel.name == ADMINISTRATION)
            .ok_or_else(|| "Proposals are made in # administration".to_owned())?;
        let config = self
            .runtime_configuration(Some(&hq.project_root), None)?
            .config
            .visions;
        let context = Context {
            visions: self.visions_list(),
            channels: self.channel_list(&hq.project_root),
            config,
            models,
        };
        Ok((hq, context))
    }

    /// The admin posts `proposal` in `# administration` (`channel_id`), its models checked against
    /// `models`; its id and seq.
    pub(crate) fn visions_propose(
        &self,
        channel_id: &str,
        mut proposal: Proposal,
        models: Vec<String>,
    ) -> Result<(String, u64), String> {
        let (_, context) = self.visions_admin_context(channel_id, models)?;
        fill_looks(&mut proposal, context.visions.len());
        if let Err(error) = check(&proposal, &context) {
            self.visions_note(
                "proposal_refused",
                json!({"vision_id": ADMIN_VISION_ID, "channel_id": channel_id, "reason": "invalid", "stage": "propose"}),
            );
            return Err(error);
        }
        let proposal_id = uuid::Uuid::new_v4().to_string();
        let counts = json!({"channels": proposal.channels.len(), "visions": proposal.visions.len(), "changes": proposal.changes.len()});
        let entry = ChannelEntry::Proposal {
            vision_id: ADMIN_VISION_ID.to_owned(),
            proposal_id: proposal_id.clone(),
            proposal,
        };
        let seq = self.channel_post(channel_id, ADMIN_VISION_ID, entry)?;
        self.visions_note(
            "proposal_posted",
            json!({"vision_id": ADMIN_VISION_ID, "channel_id": channel_id, "proposal_id": super::id_label(&proposal_id), "seq": seq, "counts": counts}),
        );
        Ok((proposal_id, seq))
    }

    /// Sets up the admin's proposal `proposal_id` in `# administration` (`channel_id`): refused unless it
    /// was never set up, is the admin's newest, the person wrote in `# administration` after it, and it
    /// still checks out.
    pub(crate) fn visions_apply_proposal(
        &self,
        channel_id: &str,
        proposal_id: &str,
        models: Vec<String>,
    ) -> Result<Vec<SetUp>, String> {
        let _one_at_a_time = self
            .inner
            .visions
            .applying
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ready = self.visions_ready_to_apply(channel_id, proposal_id, models);
        let (hq, proposal) = match ready {
            Ok(ready) => ready,
            Err(Refused(reason, message)) => {
                self.visions_note(
                    "proposal_refused",
                    json!({"vision_id": ADMIN_VISION_ID, "channel_id": channel_id, "proposal_id": super::id_label(proposal_id), "reason": reason, "stage": "apply"}),
                );
                return Err(message);
            }
        };
        let mut items = Vec::new();
        let outcome = self.visions_set_up(&hq, &proposal, &mut items);
        if !items.is_empty() {
            let entry = ChannelEntry::ProposalApplied {
                vision_id: ADMIN_VISION_ID.to_owned(),
                proposal_id: proposal_id.to_owned(),
                items: items.clone(),
            };
            self.channel_post(channel_id, ADMIN_VISION_ID, entry)?;
        }
        let count = |action| items.iter().filter(|item| item.action == action).count();
        self.visions_note(
            "proposal_applied",
            json!({"vision_id": ADMIN_VISION_ID, "channel_id": channel_id, "proposal_id": super::id_label(proposal_id),
                "created": count(SetUpAction::Created), "changed": count(SetUpAction::Changed),
                "outcome": if outcome.is_ok() { "applied" } else { "partial" }}),
        );
        outcome.map(|()| items)
    }

    /// `# administration` and the proposal when it may be set up now.
    fn visions_ready_to_apply(
        &self,
        channel_id: &str,
        proposal_id: &str,
        models: Vec<String>,
    ) -> Result<(Channel, Proposal), Refused> {
        let invalid = |message: String| Refused("invalid", message);
        let (hq, context) = self
            .visions_admin_context(channel_id, models)
            .map_err(invalid)?;
        let found = self
            .visions_find_proposal(channel_id, proposal_id)
            .map_err(|message| Refused("unknown", message))?;
        if found.applied {
            return Err(Refused(
                "applied",
                "This proposal was already set up".to_owned(),
            ));
        }
        // The person's reply after a newer proposal is to that one, not this.
        if found.superseded {
            let message = themis_core::prompts::AGENT_CLI_ADMIN_SUPERSEDED.to_owned();
            return Err(Refused("superseded", message));
        }
        if !found.replied {
            let message = themis_core::prompts::AGENT_CLI_ADMIN_NO_REPLY.to_owned();
            return Err(Refused("no_reply", message));
        }
        check(&found.proposal, &context).map_err(invalid)?;
        Ok((hq, found.proposal))
    }

    /// Looks back through `# administration`, up to [`SCAN_BYTES`], for the admin's proposal `proposal_id`.
    fn visions_find_proposal(&self, channel_id: &str, proposal_id: &str) -> Result<Found, String> {
        let unknown = || format!("Unknown proposal {proposal_id:?}; propose it again");
        let (mut before, mut scanned) = (None, 0);
        let (mut replied, mut applied, mut superseded) = (false, false, false);
        loop {
            let page = self.channel_page(channel_id, before, PAGE_BYTES)?;
            for line in page.entries.into_iter().rev() {
                match line.entry {
                    ChannelEntry::Person { .. } => replied = true,
                    ChannelEntry::ProposalApplied {
                        proposal_id: id, ..
                    } if id == proposal_id => {
                        applied = true;
                    }
                    ChannelEntry::Proposal {
                        vision_id,
                        proposal_id: id,
                        proposal,
                    } if id == proposal_id && is_admin(&vision_id) => {
                        return Ok(Found {
                            proposal,
                            replied,
                            applied,
                            superseded,
                        });
                    }
                    ChannelEntry::Proposal { vision_id, .. } if is_admin(&vision_id) => {
                        superseded = true;
                    }
                    _ => {}
                }
            }
            scanned += PAGE_BYTES;
            match page.next_before {
                Some(seq) if scanned < SCAN_BYTES => before = Some(seq),
                _ => return Err(unknown()),
            }
        }
    }

    /// Makes and changes what `proposal` says, noting each in `items` as it is done.
    fn visions_set_up(
        &self,
        hq: &Channel,
        proposal: &Proposal,
        items: &mut Vec<SetUp>,
    ) -> Result<(), String> {
        for new in &proposal.visions {
            let saved = self.vision_save(new_profile(new), None)?;
            items.push(set_up(
                SetUpKind::Vision,
                &saved.id,
                &saved.name,
                SetUpAction::Created,
            ));
        }
        // Read again: members named by a new Vision's name resolve to the saved Vision.
        let context = Context {
            visions: self.visions_list(),
            channels: self.channel_list(&hq.project_root),
            config: VisionsConfig::default(),
            models: Vec::new(),
        };
        let none = HashSet::new();
        let ids = |names: &[String]| -> Result<Vec<String>, String> {
            names
                .iter()
                .map(|name| member(&context, &none, name))
                .collect()
        };
        for new in &proposal.channels {
            let spec = Channel {
                id: String::new(),
                name: new.name.trim().to_owned(),
                topic: new.topic.clone(),
                project_root: hq.project_root.clone(),
                approval_mode: ApprovalMode::Custom,
                members: ids(&new.members)?,
                chief: String::new(),
                autonomy: None,
                custom: None,
                daily_budget_usd: None,
                revision: String::new(),
            };
            let saved = self.channel_save(spec, None)?;
            items.push(set_up(
                SetUpKind::Channel,
                &saved.id,
                &saved.name,
                SetUpAction::Created,
            ));
        }
        for change in &proposal.changes {
            let item = match (&change.vision, &change.channel) {
                (Some(name), _) => self.visions_change_vision(change, context.vision(name)?)?,
                (_, Some(name)) => {
                    let channel = context.channel(name)?;
                    let (add, remove) = (ids(&change.add_members)?, ids(&change.remove_members)?);
                    let changed = applied_to_channel(change, channel, &add, &remove);
                    let saved = self.channel_save(changed, Some(&channel.revision))?;
                    set_up(
                        SetUpKind::Channel,
                        &saved.id,
                        &saved.name,
                        SetUpAction::Changed,
                    )
                }
                _ => continue,
            };
            items.push(item);
        }
        Ok(())
    }

    fn visions_change_vision(&self, change: &Change, vision: &Vision) -> Result<SetUp, String> {
        let mut changed = applied_to_vision(change, vision);
        changed.paused = vision.paused;
        let mut saved = vision.clone();
        if changed != *vision {
            saved = self.vision_save(changed, Some(&vision.revision))?;
        }
        if let Some(paused) = change.paused {
            saved = self.visions_pause(&saved.id, paused)?;
        }
        Ok(set_up(
            SetUpKind::Vision,
            &saved.id,
            &saved.name,
            SetUpAction::Changed,
        ))
    }
}

fn set_up(kind: SetUpKind, id: &str, name: &str, action: SetUpAction) -> SetUp {
    SetUp {
        kind,
        id: id.to_owned(),
        name: name.to_owned(),
        action,
    }
}

#[cfg(test)]
#[path = "admin_tests.rs"]
mod tests;
