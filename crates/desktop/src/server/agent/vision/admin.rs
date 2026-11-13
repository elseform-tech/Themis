//! The admin Vision's grant: it proposes setups in its `# hq` and sets one up after the person
//! replied (`state/visions/admin.rs` checks the rest), and reads the Visions and its project's
//! channels. It drives no chat and manages no integration; everything else goes by the
//! ordinary rules of a run's credential (its own chat's reads, status).

use super::*;
use crate::state::Proposal;

/// The admin's own writes, which only its grant has.
pub(in crate::server::agent) const ADMIN_WRITES: &[&str] =
    &["propose_visions_setup", "apply_proposal"];

/// The admin grant's own decision on a call, like [`super::refusal`]'s.
pub(super) fn refusal(scope: &VisionScope, method: &str, args: &Value) -> Option<Option<Refusal>> {
    let refuse = || Some(refused(Reason::MethodNotAllowed, method_label(method)));
    match method {
        "propose_visions_setup" => {
            let ours = args.get("channelId").and_then(Value::as_str) == Some(&scope.channel_id);
            Some(if ours { None } else { refuse() })
        }
        "apply_proposal" | "list_visions" => Some(None),
        "list_channels" => Some((!in_channel_project(scope, args)).then(other_project)),
        "plugin_action" => Some(refuse()),
        _ if VISION_DRIVE.contains(&method) => Some(refuse()),
        _ => None,
    }
}

/// Runs one of [`ADMIN_WRITES`] for the admin's `# hq`, then announces it: a proposal with the
/// channel and the admin, a setup with no ids, since it may have made channels and Visions
/// anywhere in the list.
pub(super) fn dispatch(
    state: &AppState,
    sink: &Arc<dyn EventSink>,
    scope: &VisionScope,
    method: &str,
    args: &Value,
) -> Result<Value, String> {
    if method == "apply_proposal" {
        let proposal_id: String = arg(args, "proposalId")?;
        let applied = state.visions_apply_proposal(&scope.channel_id, &proposal_id);
        // Even a setup that failed part way may have made something.
        sink.emit_visions_changed(&VisionsChange::default());
        return applied.map(|items| json!({"items": items}));
    }
    let proposal: Proposal = arg(args, "proposal")?;
    let (id, seq) = state.visions_propose(&scope.channel_id, proposal)?;
    sink.emit_visions_changed(&VisionsChange {
        channel_id: Some(scope.channel_id.clone()),
        vision_id: Some(scope.vision_id.clone()),
    });
    Ok(json!({"proposalId": id, "seq": seq}))
}
