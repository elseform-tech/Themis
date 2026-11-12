//! What the person does to Visions, channels and the chats Visions own: write in a channel,
//! stop, take over and hand back a chat, pause and delete. Logged as metadata only.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};

use super::{Channel, ChannelEntry, Hold, NoticeKind, OwnedKind, Owner, Vision, WakeReason};
use crate::sink::EventSink;
use crate::state::{canonical_project_dir, AppState};
use crate::types::ThreadInfo;

const NOT_OWNED: &str = "This chat does not belong to a Vision";

/// True when `text` mentions `name` as `@Name` (any case), not as the start of a longer word.
fn mentions(text: &str, name: &str) -> bool {
    let text = text.to_lowercase();
    let wanted = format!("@{}", name.to_lowercase());
    text.match_indices(&wanted).any(|(at, found)| {
        text[at + found.len()..]
            .chars()
            .next()
            .is_none_or(|next| !next.is_alphanumeric() && next != '_' && next != '-')
    })
}

impl AppState {
    fn visions_note(&self, event: &str, fields: Value) {
        let _ = self
            .inner
            .diagnostics
            .append("visions", event, "info", fields);
    }

    /// The chats some Vision owns, to keep them out of the plain list.
    pub(crate) fn visions_owned_ids(&self) -> HashSet<String> {
        self.owners_where(|_| true)
            .into_iter()
            .map(|owner| owner.thread_id)
            .collect()
    }

    /// The work chats Visions started in `channel_id`, each with its owner record.
    pub async fn visions_channel_threads(&self, channel_id: &str) -> Result<Vec<Value>, String> {
        let owners = self
            .owners_where(|owner| owner.channel_id == channel_id && owner.kind == OwnedKind::Work);
        let threads = self.inner.threads.read().await;
        let mut listed = Vec::with_capacity(owners.len());
        for owner in owners {
            let Some(record) = threads.get(&owner.thread_id) else {
                continue;
            };
            let info: ThreadInfo = self.info_of(record)?;
            let mut value = serde_json::to_value(info).map_err(|e| e.to_string())?;
            value["owner"] = serde_json::to_value(&owner).map_err(|e| e.to_string())?;
            listed.push(value);
        }
        Ok(listed)
    }

    /// The channels of the project at `project_root`, which is resolved like `list_threads`
    /// resolves it.
    pub fn visions_project_channels(&self, project_root: &str) -> Result<Vec<Channel>, String> {
        let root = canonical_project_dir(project_root)?;
        Ok(self.channel_list(&root))
    }

    /// Saves a channel with its project folder resolved like `list_threads` resolves it.
    pub fn visions_save_channel(
        &self,
        mut spec: Channel,
        expected: Option<&str>,
    ) -> Result<Channel, String> {
        spec.project_root = canonical_project_dir(&spec.project_root.to_string_lossy())?;
        self.channel_save(spec, expected)
    }

    /// The person wrote in a channel: the log gets it, chats the person stopped there may be
    /// picked up again (taken-over ones stay the person's), and the Visions it is for wake:
    /// those it mentions as `@Name`, or the only member when it mentions none.
    pub fn visions_post(
        &self,
        sink: &Arc<dyn EventSink>,
        channel_id: &str,
        text: &str,
    ) -> Result<u64, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("Write a message first".to_owned());
        }
        let channel = self
            .channel_get(channel_id)
            .ok_or_else(|| "Unknown channel".to_owned())?;
        let seq = self.channel_append(
            channel_id,
            ChannelEntry::Person {
                text: text.to_owned(),
            },
        )?;
        let stopped = self.owners_where(|owner| {
            owner.channel_id == channel_id && owner.hold == Hold::StoppedByPerson
        });
        for owner in &stopped {
            self.owner_hold(&owner.thread_id, |_| Hold::None)?;
        }
        let members: Vec<Vision> = channel
            .members
            .iter()
            .filter_map(|id| self.vision_get(id))
            .collect();
        let mut woken: Vec<&Vision> = members
            .iter()
            .filter(|vision| mentions(text, &vision.name))
            .collect();
        if woken.is_empty() && members.len() == 1 {
            woken.push(&members[0]);
        }
        for vision in &woken {
            self.visions_wake(sink, channel_id, &vision.id, WakeReason::Mentioned);
        }
        self.visions_note(
            "channel_posted",
            json!({"channel_id": channel_id, "seq": seq, "released": stopped.len(), "woken": woken.len()}),
        );
        Ok(seq)
    }

    /// The person stopped a chat: a Vision's work chat is held until the person writes in its
    /// channel or hands it back (a taken-over chat stays taken over), and the channel says so.
    pub(crate) fn visions_person_stopped(&self, thread_id: &str) {
        let Some(owner) = self.owner_of(thread_id) else {
            return;
        };
        // Stopped again: the channel already says so.
        if owner.kind != OwnedKind::Work || owner.hold == Hold::StoppedByPerson {
            return;
        }
        let held = self.owner_hold(thread_id, |owner| match owner.hold {
            Hold::TakenOver => Hold::TakenOver,
            _ => Hold::StoppedByPerson,
        });
        let _ = self.channel_append(
            &owner.channel_id,
            ChannelEntry::Notice {
                kind: NoticeKind::Stopped,
                vision_id: Some(owner.vision_id.clone()),
                thread_id: Some(thread_id.to_owned()),
            },
        );
        self.visions_note(
            "chat_held",
            json!({"thread_id": thread_id, "channel_id": owner.channel_id, "hold": "stopped_by_person", "saved": held.is_ok()}),
        );
    }

    /// The person wrote in a Vision's work chat: the chat is theirs until they hand it back.
    pub(crate) fn visions_person_took_over(&self, thread_id: &str) {
        let Some(owner) = self.owner_of(thread_id) else {
            return;
        };
        if owner.kind != OwnedKind::Work || owner.hold == Hold::TakenOver {
            return;
        }
        let held = self.owner_hold(thread_id, |_| Hold::TakenOver);
        self.visions_note(
            "chat_held",
            json!({"thread_id": thread_id, "channel_id": owner.channel_id, "hold": "taken_over", "saved": held.is_ok()}),
        );
    }

    /// A discarded chat no longer belongs to anyone.
    pub(crate) fn visions_chat_discarded(&self, thread_id: &str) {
        if self.owner_of(thread_id).is_some() {
            let _ = self.owner_release(thread_id);
        }
    }

    /// The person gives a work chat back to its Vision, which wakes to read what happened.
    pub fn visions_hand_back(
        &self,
        sink: &Arc<dyn EventSink>,
        thread_id: &str,
    ) -> Result<Owner, String> {
        let owner = self
            .owner_of(thread_id)
            .filter(|owner| owner.kind == OwnedKind::Work)
            .ok_or_else(|| NOT_OWNED.to_owned())?;
        if owner.hold == Hold::None {
            return Err(
                "This chat is already its Vision's; there is nothing to hand back".to_owned(),
            );
        }
        let owner = self.owner_hold(thread_id, |_| Hold::None)?.unwrap_or(owner);
        self.channel_append(
            &owner.channel_id,
            ChannelEntry::Notice {
                kind: NoticeKind::HandedBack,
                vision_id: Some(owner.vision_id.clone()),
                thread_id: Some(thread_id.to_owned()),
            },
        )?;
        self.visions_wake(
            sink,
            &owner.channel_id,
            &owner.vision_id,
            WakeReason::HandedBack(thread_id.to_owned()),
        );
        self.visions_note(
            "chat_handed_back",
            json!({"thread_id": thread_id, "channel_id": owner.channel_id}),
        );
        Ok(owner)
    }

    /// Pauses or resumes a Vision. Pausing stops the runs of the chats it owns (not those the
    /// person took over); its channels say so either way.
    pub fn visions_pause(&self, vision_id: &str, paused: bool) -> Result<Vision, String> {
        let vision = self
            .vision_get(vision_id)
            .ok_or_else(|| "Unknown Vision".to_owned())?;
        if vision.paused == paused {
            return Ok(vision);
        }
        let revision = vision.revision.clone();
        let saved = self.vision_save(Vision { paused, ..vision }, Some(&revision))?;
        let stopped = if paused {
            // A chat the person took over runs for them, not for the Vision.
            self.visions_stop_owned(|owner| {
                owner.vision_id == vision_id && owner.hold != Hold::TakenOver
            })
        } else {
            0
        };
        let kind = if paused {
            NoticeKind::Paused
        } else {
            NoticeKind::Resumed
        };
        for channel in self.channels_all() {
            if channel.members.iter().any(|member| member == vision_id) {
                let _ = self.channel_append(
                    &channel.id,
                    ChannelEntry::Notice {
                        kind,
                        vision_id: Some(vision_id.to_owned()),
                        thread_id: None,
                    },
                );
            }
        }
        self.visions_note(
            "vision_paused",
            json!({"vision_id": vision_id, "paused": paused, "stopped": stopped}),
        );
        Ok(saved)
    }

    /// Deletes a Vision: its chats are stopped and become ordinary chats; channel logs stay.
    pub fn visions_delete(&self, vision_id: &str) -> Result<(), String> {
        if self.vision_get(vision_id).is_none() {
            return Err("Unknown Vision".to_owned());
        }
        let released = self.visions_release_owned(|owner| owner.vision_id == vision_id)?;
        self.vision_remove(vision_id)?;
        self.visions_note(
            "vision_deleted",
            json!({"vision_id": vision_id, "released": released}),
        );
        Ok(())
    }

    /// Deletes a channel: its chats are stopped and become ordinary chats; its log stays.
    pub fn visions_delete_channel(&self, channel_id: &str) -> Result<(), String> {
        if self.channel_get(channel_id).is_none() {
            return Err("Unknown channel".to_owned());
        }
        let released = self.visions_release_owned(|owner| owner.channel_id == channel_id)?;
        self.channel_remove(channel_id)?;
        self.visions_note(
            "channel_deleted",
            json!({"channel_id": channel_id, "released": released}),
        );
        Ok(())
    }

    /// Asks the running chats `which` selects to stop; returns how many were running.
    fn visions_stop_owned(&self, which: impl Fn(&Owner) -> bool) -> usize {
        self.owners_where(which)
            .iter()
            .filter(|owner| self.stop_run(&owner.thread_id).is_ok())
            .count()
    }

    /// Stops the chats `which` selects (not those the person took over, which run for them)
    /// and forgets their owners; returns how many.
    fn visions_release_owned(&self, which: impl Fn(&Owner) -> bool) -> Result<usize, String> {
        let owners = self.owners_where(which);
        for owner in &owners {
            if owner.hold != Hold::TakenOver {
                let _ = self.stop_run(&owner.thread_id);
            }
            self.owner_release(&owner.thread_id)?;
        }
        Ok(owners.len())
    }
}

#[cfg(test)]
mod tests {
    use super::mentions;

    #[test]
    fn mentions_match_the_name_in_any_case_but_not_a_longer_word() {
        assert!(mentions("hey @wren, look", "Wren"));
        assert!(mentions("@WREN", "Wren"));
        assert!(mentions("ask @Ada Lovelace now", "Ada Lovelace"));
        assert!(!mentions("hey @wrenna", "Wren"));
        assert!(!mentions("hey wren", "Wren"));
        assert!(mentions("@wrenna and @wren", "Wren"));
    }
}
