//! Requests between channels. A chief asks another channel's chief in the same project for
//! something; the receiving chief accepts (taking on new work: the `scope` decision of its
//! channel's level) or declines (always allowed) — or, beyond its level, leaves it for the
//! person, who answers in that channel. Each channel shows its side of the exchange; the asking
//! chief wakes on the answer. The receiving channel pays for the work, its chief's wake
//! included. Each channel sends at most `visions.requests_per_day` a local day; past that the
//! call is refused and the channel is told once.
//!
//! Requests are kept in `requests.json` under the Visions store, written by the server only,
//! under the Visions lock; answered ones go after [`KEEP_ANSWERED_DAYS`]. Diagnostics carry
//! ids and outcomes, never the ask or the note.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use themis_core::visions::Decision;

use super::admin::ADMINISTRATION;
use super::store::{read_store, write_store};
use super::{AppState, ChannelEntry, NoticeKind, WakeReason};
use crate::sink::EventSink;

const REQUESTS_FILE: &str = "requests.json";
/// Answered requests are kept this long, then dropped.
const KEEP_ANSWERED_DAYS: i64 = 7;
/// Most characters of an ask or a note.
const MAX_TEXT_CHARS: usize = 2000;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// One request between channels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Request {
    id: String,
    from_channel: String,
    to_channel: String,
    at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    answered: Option<Answered>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Answered {
    accepted: bool,
    at_ms: i64,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn midnight_ms() -> i64 {
    let today = chrono::Local::now().date_naive();
    today
        .and_hms_opt(0, 0, 0)
        .and_then(|midnight| midnight.and_local_timezone(chrono::Local).earliest())
        .map_or(0, |at| at.timestamp_millis())
}

fn text(value: &str, what: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_TEXT_CHARS {
        return Err(format!(
            "The {what} is some text of at most {MAX_TEXT_CHARS} characters"
        ));
    }
    Ok(value.to_owned())
}

impl AppState {
    fn requests_load(&self) -> Result<Vec<Request>, String> {
        read_store(&self.inner.visions_dir, REQUESTS_FILE)
            .map_err(|error| self.visions_failed(error))
    }

    fn requests_save(&self, requests: &[Request]) -> Result<(), String> {
        let keep_after = now_ms() - KEEP_ANSWERED_DAYS * DAY_MS;
        let kept: Vec<&Request> = requests
            .iter()
            .filter(|request| {
                request
                    .answered
                    .as_ref()
                    .is_none_or(|a| a.at_ms >= keep_after)
            })
            .collect();
        write_store(&self.inner.visions_dir, REQUESTS_FILE, &kept)
            .map_err(|error| self.visions_failed(error))
    }

    /// `from_chief`, leading `from_channel`, asks `to_channel`'s chief; returns the request id.
    pub(crate) fn visions_request(
        &self,
        sink: &Arc<dyn EventSink>,
        from_channel: &str,
        from_chief: &str,
        to_channel: &str,
        ask: &str,
    ) -> Result<String, String> {
        let ask = text(ask, "request")?;
        let from = self.channel_get(from_channel).ok_or("Unknown channel")?;
        let to = self
            .channel_get(to_channel)
            .filter(|to| to.id != from.id && to.name != ADMINISTRATION)
            .filter(|to| to.project_root == from.project_root)
            .ok_or_else(|| "Ask another channel of this project".to_owned())?;
        if to.chief.is_empty() {
            return Err(format!("# {} has no chief of staff to ask", to.name));
        }
        let cap = self
            .runtime_configuration(Some(&from.project_root), None)?
            .config
            .visions
            .requests_per_day;
        let id = uuid::Uuid::new_v4().to_string();
        {
            // Counted and recorded under one lock, so two calls cannot both pass the cap.
            let _guard = self.visions_cache();
            let mut requests = self.requests_load()?;
            let today = midnight_ms();
            let sent = requests
                .iter()
                .filter(|request| request.from_channel == from.id && request.at_ms >= today)
                .count();
            if sent >= cap as usize {
                drop(_guard);
                self.visions_request_cap(&from.id, &from.chief);
                return Err(themis_core::prompts::fill(
                    themis_core::prompts::AGENT_CLI_REQUEST_CAP,
                    &[("cap", &cap.to_string())],
                ));
            }
            requests.push(Request {
                id: id.clone(),
                from_channel: from.id.clone(),
                to_channel: to.id.clone(),
                at_ms: now_ms(),
                answered: None,
            });
            self.requests_save(&requests)?;
        }
        self.channel_post(
            &from.id,
            from_chief,
            ChannelEntry::RequestSent {
                vision_id: from_chief.to_owned(),
                request_id: id.clone(),
                channel_id: to.id.clone(),
                ask: ask.clone(),
            },
        )?;
        self.channel_post(
            &to.id,
            &to.chief,
            ChannelEntry::RequestReceived {
                vision_id: from_chief.to_owned(),
                request_id: id.clone(),
                channel_id: from.id.clone(),
                ask,
            },
        )?;
        self.visions_note(
            "request_sent",
            json!({"request_id": id, "from_channel": from.id, "to_channel": to.id}),
        );
        // The receiving channel pays for its chief's wake, whatever it answers.
        self.visions_wake(sink, &to.id, &to.chief, WakeReason::Request(id.clone()));
        Ok(id)
    }

    /// The sending channel reached today's cap: told once a day.
    fn visions_request_cap(&self, channel_id: &str, chief: &str) {
        let key = format!("request_cap:{channel_id}");
        if !self.visions_notice_claim_today(&key) {
            return;
        }
        let entry = ChannelEntry::Notice {
            kind: NoticeKind::RequestCap,
            vision_id: (!chief.is_empty()).then(|| chief.to_owned()),
            thread_id: None,
            unpriced_requests: None,
            reason: None,
        };
        let _ = self.channel_post(channel_id, chief, entry);
        self.visions_note("request_cap", json!({"channel_id": channel_id}));
    }

    /// The receiving chief's answer: declining is always its own; accepting takes on new work
    /// (the `scope` decision of its channel's level).
    pub(crate) fn visions_answer_request(
        &self,
        sink: &Arc<dyn EventSink>,
        channel_id: &str,
        chief: &str,
        request_id: &str,
        accept: bool,
        note: &str,
    ) -> Result<(), String> {
        let channel = self.channel_get(channel_id).ok_or("Unknown channel")?;
        if accept {
            let config = self
                .runtime_configuration(Some(&channel.project_root), None)?
                .config;
            let decides = super::vision_decisions(&channel, &config.visions);
            if !decides.contains(&Decision::Scope) {
                return Err(themis_core::prompts::AGENT_CLI_REQUEST_ABOVE_LEVEL.to_owned());
            }
        }
        self.answer_request(sink, channel_id, chief, request_id, accept, note)
    }

    /// The person accepts or declines a request their channel's chief could not or did not.
    pub fn visions_person_answers_request(
        &self,
        sink: &Arc<dyn EventSink>,
        request_id: &str,
        accept: bool,
    ) -> Result<(), String> {
        let to = self
            .requests_load()?
            .into_iter()
            .find(|request| request.id == request_id)
            .map(|request| request.to_channel)
            .ok_or("This request is no longer open")?;
        let chief = self
            .channel_get(&to)
            .map(|channel| channel.chief)
            .unwrap_or_default();
        self.answer_request(sink, &to, &chief, request_id, accept, "")
    }

    /// Records the answer once, shows it in both channels and wakes the asking chief.
    fn answer_request(
        &self,
        sink: &Arc<dyn EventSink>,
        channel_id: &str,
        by: &str,
        request_id: &str,
        accept: bool,
        note: &str,
    ) -> Result<(), String> {
        let note = note.trim();
        if note.chars().count() > MAX_TEXT_CHARS {
            return Err(format!("The note is at most {MAX_TEXT_CHARS} characters"));
        }
        let request = {
            let _guard = self.visions_cache();
            let mut requests = self.requests_load()?;
            let request = requests
                .iter_mut()
                .find(|request| request.id == request_id && request.to_channel == channel_id)
                .filter(|request| request.answered.is_none())
                .ok_or("This request is no longer open")?;
            request.answered = Some(Answered {
                accepted: accept,
                at_ms: now_ms(),
            });
            let request = request.clone();
            self.requests_save(&requests)?;
            request
        };
        for (here, there) in [
            (&request.to_channel, &request.from_channel),
            (&request.from_channel, &request.to_channel),
        ] {
            let entry = ChannelEntry::RequestAnswered {
                vision_id: by.to_owned(),
                request_id: request.id.clone(),
                channel_id: there.clone(),
                accepted: accept,
                note: note.to_owned(),
            };
            self.channel_post(here, by, entry)?;
        }
        self.visions_note(
            "request_answered",
            json!({"request_id": request.id, "accepted": accept, "by_person": by.is_empty()}),
        );
        if let Some(asker) = self.channel_get(&request.from_channel).map(|c| c.chief) {
            let answered = WakeReason::RequestAnswered(request.id.clone());
            self.visions_wake(sink, &request.from_channel, &asker, answered);
        }
        Ok(())
    }
}
