use super::*;
use crate::visions::{vision_decides, Autonomy, Decision};

/// Visions (`visions`, Settings › Visions): agents that drive ordinary chats; see
/// [`crate::visions`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VisionsConfig {
    /// Allow Visions to wake and drive chats. A project or run layer may only set it to false.
    pub enabled: bool,
    /// How much a Vision decides without asking: `ask`, `small`, `most`, `nearly_all` or
    /// `custom` (the rows in `custom`). A project or run layer may only lower it.
    pub autonomy: Autonomy,
    /// With `autonomy: "custom"`, which decisions (`wording`, `tests`, `undoable`, `retry`,
    /// `scope`, `delete`, `dependency`) the Vision makes itself; `external` and `spend` are
    /// always the person's. A project or run layer may only set rows to false.
    pub custom: BTreeMap<String, bool>,
    /// Estimated dollars all Visions together may spend in a day. A project may only lower it.
    pub daily_budget_usd: f64,
    /// Most times a Vision wakes on one thing before it hands back to the person (1-20). A
    /// project or run layer may only lower it.
    pub max_wakes: u32,
    /// Most requests a channel's chief may send other channels in a day (1-100). A project or
    /// run layer may only lower it.
    pub requests_per_day: u32,
    /// Most routines one chief may keep (1-50). A project or run layer may only lower it.
    pub routines_per_chief: u32,
    /// Shortest time between two runs of one routine, in minutes (60-1440). A project or run
    /// layer may only raise it.
    pub routine_min_minutes: u32,
    /// Local hours when routines and other agent-started wakes wait (the person's messages
    /// always go through); `null` for none. User file only.
    pub quiet_hours: Option<QuietHours>,
}

/// `visions.quiet_hours`: from `start` to `end`, each `HH:MM` local time (may cross midnight).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuietHours {
    pub start: String,
    pub end: String,
}

impl QuietHours {
    /// Minutes after midnight of `HH:MM`, or `None` when it is not a time of day.
    pub fn minutes(time: &str) -> Option<u32> {
        let (hours, minutes) = time.split_once(':')?;
        if hours.len() != 2 || minutes.len() != 2 {
            return None;
        }
        let (hours, minutes): (u32, u32) = (hours.parse().ok()?, minutes.parse().ok()?);
        (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
    }

    /// Whether `minute` (after midnight) is quiet: from `start` up to `end`, crossing midnight
    /// when `end` is earlier than `start`. Times that cannot be read are never quiet.
    pub fn contains(&self, minute: u32) -> bool {
        match (Self::minutes(&self.start), Self::minutes(&self.end)) {
            (Some(start), Some(end)) if start < end => (start..end).contains(&minute),
            (Some(start), Some(end)) if start > end => minute >= start || minute < end,
            _ => false,
        }
    }
}

/// Highest `visions.requests_per_day`.
pub const MAX_VISION_REQUESTS: u32 = 100;
/// Highest `visions.routines_per_chief`.
pub const MAX_VISION_ROUTINES: u32 = 50;
/// Shortest and longest `visions.routine_min_minutes`.
pub const ROUTINE_MINUTES: std::ops::RangeInclusive<u32> = 60..=1440;

/// Highest `visions.daily_budget_usd`.
pub const MAX_VISIONS_BUDGET_USD: f64 = 100.0;
/// Lowest `visions.daily_budget_usd`.
pub const MIN_VISIONS_BUDGET_USD: f64 = 0.1;
/// Highest `visions.max_wakes`.
pub const MAX_VISION_WAKES: u32 = 20;

impl Default for VisionsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            autonomy: Autonomy::Small,
            custom: BTreeMap::new(),
            daily_budget_usd: 1.0,
            max_wakes: 6,
            requests_per_day: 10,
            routines_per_chief: 10,
            routine_min_minutes: 60,
            quiet_hours: None,
        }
    }
}

/// A project or run layer may only lower autonomy, the budget and the wakes, turn Visions
/// off, and set custom rows to false.
fn check_visions_restriction(current: &VisionsConfig, layer: &Value, origin: &str) -> Result<()> {
    if let Some(value) = layer.pointer("/visions/enabled") {
        ensure!(
            value == &Value::Bool(false),
            "the {origin} configuration may only set visions.enabled to false"
        );
    }
    if let Some(value) = layer.pointer("/visions/autonomy") {
        let requested: Autonomy = serde_json::from_value(value.clone()).map_err(|_| {
            anyhow::anyhow!("visions.autonomy must be ask/small/most/nearly_all/custom")
        })?;
        let allowed = match (requested.rank(), current.autonomy.rank()) {
            (Some(0), _) => true,
            (Some(new), Some(old)) => new <= old,
            (Some(_), None) => Decision::ALL.into_iter().all(|d| {
                !vision_decides(requested, &BTreeMap::new(), d)
                    || vision_decides(Autonomy::Custom, &current.custom, d)
            }),
            (None, _) => current.autonomy == Autonomy::Custom,
        };
        ensure!(
            allowed,
            "the {origin} configuration may only lower visions.autonomy"
        );
    }
    if let Some(rows) = layer.pointer("/visions/custom").and_then(Value::as_object) {
        ensure!(
            rows.values().all(|row| row == &Value::Bool(false)),
            "the {origin} configuration may only set visions.custom rows to false"
        );
    }
    if let Some(value) = layer.pointer("/visions/daily_budget_usd") {
        ensure!(
            value
                .as_f64()
                .is_some_and(|new| new <= current.daily_budget_usd),
            "the {origin} configuration may only lower visions.daily_budget_usd"
        );
    }
    check_limits_restriction(current, layer, origin)?;
    if let Some(value) = layer.pointer("/visions/max_wakes") {
        ensure!(
            value
                .as_u64()
                .is_some_and(|new| new <= u64::from(current.max_wakes)),
            "the {origin} configuration may only lower visions.max_wakes"
        );
    }
    Ok(())
}

/// Requests and routines: a project or run layer may only lower the counts, raise the shortest
/// interval, and never set quiet hours (they are the person's).
fn check_limits_restriction(current: &VisionsConfig, layer: &Value, origin: &str) -> Result<()> {
    let lowered = |field: &str, now: u32| {
        layer
            .pointer(&format!("/visions/{field}"))
            .is_none_or(|value| value.as_u64().is_some_and(|new| new <= u64::from(now)))
    };
    ensure!(
        lowered("requests_per_day", current.requests_per_day)
            && lowered("routines_per_chief", current.routines_per_chief),
        "the {origin} configuration may only lower visions.requests_per_day and visions.routines_per_chief"
    );
    ensure!(
        layer
            .pointer("/visions/routine_min_minutes")
            .is_none_or(|value| value
                .as_u64()
                .is_some_and(|new| new >= u64::from(current.routine_min_minutes))),
        "the {origin} configuration may only raise visions.routine_min_minutes"
    );
    ensure!(
        layer.pointer("/visions/quiet_hours").is_none(),
        "visions.quiet_hours is set in your own runtime.jsonc only"
    );
    Ok(())
}

fn validate_visions(config: &RuntimeConfig) -> Result<()> {
    let visions = &config.visions;
    for row in visions.custom.keys() {
        ensure!(
            matches!(Decision::from_id(row), Some(d) if !matches!(d, Decision::External | Decision::Spend)),
            "visions.custom has no row \"{row}\" (rows: wording, tests, undoable, retry, scope, delete, dependency)"
        );
    }
    ensure!(
        visions.daily_budget_usd.is_finite()
            && (MIN_VISIONS_BUDGET_USD..=MAX_VISIONS_BUDGET_USD)
                .contains(&visions.daily_budget_usd),
        "visions.daily_budget_usd must be {MIN_VISIONS_BUDGET_USD}..{MAX_VISIONS_BUDGET_USD}"
    );
    ensure!(
        (1..=MAX_VISION_WAKES).contains(&visions.max_wakes),
        "visions.max_wakes must be 1..{MAX_VISION_WAKES}"
    );
    ensure!(
        (1..=MAX_VISION_REQUESTS).contains(&visions.requests_per_day),
        "visions.requests_per_day must be 1..{MAX_VISION_REQUESTS}"
    );
    ensure!(
        (1..=MAX_VISION_ROUTINES).contains(&visions.routines_per_chief),
        "visions.routines_per_chief must be 1..{MAX_VISION_ROUTINES}"
    );
    ensure!(
        ROUTINE_MINUTES.contains(&visions.routine_min_minutes),
        "visions.routine_min_minutes must be 60..1440"
    );
    if let Some(quiet) = &visions.quiet_hours {
        ensure!(
            QuietHours::minutes(&quiet.start).is_some()
                && QuietHours::minutes(&quiet.end).is_some(),
            "visions.quiet_hours start and end are HH:MM times of day"
        );
        ensure!(
            quiet.start != quiet.end,
            "visions.quiet_hours start and end are different times"
        );
    }
    Ok(())
}

/// Restriction checks for the memory and visions blocks of a project or run layer, against the
/// configuration merged so far.
pub(super) fn check_memory_and_visions_restriction(
    merged: &Value,
    layer: &Value,
    origin: &str,
) -> Result<()> {
    check_memory_restriction(layer, origin)?;
    let current: VisionsConfig = serde_json::from_value(merged["visions"].clone())
        .context("invalid visions configuration")?;
    check_visions_restriction(&current, layer, origin)
}

/// Validation of the memory and visions blocks.
pub(super) fn validate_memory_and_visions(config: &RuntimeConfig) -> Result<()> {
    validate_memory(config)?;
    validate_visions(config)
}
