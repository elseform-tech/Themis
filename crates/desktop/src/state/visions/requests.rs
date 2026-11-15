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

use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::admin::ADMINISTRATION;
use super::store::{read_store, write_store};
use super::{AppState, ChannelEntry, WakeReason};
use crate::sink::EventSink;

const REQUESTS_FILE: &str = "requests.json";
/// Answered requests are kept this long, then dropped.
const KEEP_ANSWERED_DAYS: i64 = 7;
/// Open requests are kept this long (or until their channel goes), then dropped.
const KEEP_OPEN_DAYS: i64 = 30;
/// Most characters of an ask or a note.
const MAX_TEXT_CHARS: usize = 2000;
/// How long a request waiting for its result keeps its pair of channels from asking again.
const OPEN_WAIT_MS: i64 = 6 * 3_600_000;
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
    /// The receiving chief sent its result back (once).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    result_sent: bool,
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
    fn channel_ids(&self) -> HashSet<String> {
        self.channels_all()
            .into_iter()
            .map(|channel| channel.id)
            .collect()
    }

    fn requests_load(&self) -> Result<Vec<Request>, String> {
        read_store(&self.inner.visions_dir, REQUESTS_FILE)
            .map_err(|error| self.visions_failed(error))
    }

    /// Saves what may still matter: open requests to a channel that still exists (for at most
    /// [`KEEP_OPEN_DAYS`]) and answered ones for [`KEEP_ANSWERED_DAYS`].
    /// `channels` are the channel ids now, read before the Visions lock is taken (reading them
    /// takes it).
    fn requests_save(
        &self,
        requests: &[Request],
        channels: &HashSet<String>,
    ) -> Result<(), String> {
        let now = now_ms();
        let kept: Vec<&Request> = requests
            .iter()
            // A request to or from a deleted channel goes with it, answered or not.
            // (None are dropped when the channels could not be read.)
            .filter(|request| {
                channels.is_empty()
                    || (channels.contains(&request.to_channel)
                        && channels.contains(&request.from_channel))
            })
            .filter(|request| match &request.answered {
                Some(answered) => answered.at_ms >= now - KEEP_ANSWERED_DAYS * DAY_MS,
                None => request.at_ms >= now - KEEP_OPEN_DAYS * DAY_MS,
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
        let id = self.record_request(&from, &to.id, true)?;
        // Recorded: the posts are best effort, so a channel that cannot record a line still gets
        // its wake (the failure is logged by the store).
        let _ = self.channel_post(
            &from.id,
            from_chief,
            ChannelEntry::RequestSent {
                vision_id: from_chief.to_owned(),
                request_id: id.clone(),
                channel_id: to.id.clone(),
                ask: ask.clone(),
            },
        );
        let _ = self.channel_post(
            &to.id,
            &to.chief,
            ChannelEntry::RequestReceived {
                vision_id: from_chief.to_owned(),
                request_id: id.clone(),
                channel_id: from.id.clone(),
                ask,
            },
        );
        self.visions_note(
            "request_sent",
            json!({"request_id": id, "from_channel": from.id, "to_channel": to.id}),
        );
        // Above Always ask, the request is the receiving team's work at once: both channels say
        // so now, and its chief wakes to do it and send the result back.
        if self.visions_takes_requests(&to) {
            let open = |request: &Request| request.answered.is_none();
            let taken = self.request_update(&to.id, &id, open, |request| {
                request.answered = Some(Answered {
                    accepted: true,
                    at_ms: now_ms(),
                });
            });
            if let Ok(request) = taken {
                self.post_answered(&request, &to.chief, true, "");
            }
        }
        // The receiving channel pays for its chief's wake, whatever it answers.
        self.visions_wake(sink, &to.id, &to.chief, WakeReason::Request(id.clone()));
        Ok(id)
    }

    /// Records a request from `from` to `to_id` and returns its id, refused at `from`'s daily
    /// cap, and with `one_open` while a request between the two still waits for its result
    /// (checked and recorded under one lock, so two calls cannot both pass).
    fn record_request(
        &self,
        from: &super::Channel,
        to_id: &str,
        one_open: bool,
    ) -> Result<String, String> {
        let cap = self
            .runtime_configuration(Some(&from.project_root), None)?
            .config
            .visions
            .requests_per_day;
        let id = uuid::Uuid::new_v4().to_string();
        let channels = self.channel_ids();
        let guard = self.visions_cache();
        let mut requests = self.requests_load()?;
        let today = midnight_ms();
        let sent = requests
            .iter()
            .filter(|request| request.from_channel == from.id && request.at_ms >= today)
            .count();
        if sent >= cap as usize {
            drop(guard);
            self.visions_request_cap(&from.id);
            return Err(themis_core::prompts::fill(
                themis_core::prompts::AGENT_CLI_REQUEST_CAP,
                &[("cap", &cap.to_string())],
            ));
        }
        // A request whose result has not come back within a few hours stops holding the pair.
        let recent = now_ms() - OPEN_WAIT_MS;
        let waiting = |request: &&Request| {
            let since = request.answered.as_ref().map_or(request.at_ms, |a| a.at_ms);
            request.from_channel == from.id
                && request.to_channel == to_id
                && !request.result_sent
                && request.answered.as_ref().is_none_or(|a| a.accepted)
                && since >= recent
        };
        if one_open && requests.iter().any(|request| waiting(&request)) {
            return Err(themis_core::prompts::AGENT_CLI_REQUEST_ALREADY_OPEN.to_owned());
        }
        requests.push(Request {
            id: id.clone(),
            from_channel: from.id.clone(),
            to_channel: to_id.to_owned(),
            at_ms: now_ms(),
            answered: None,
            result_sent: false,
        });
        self.requests_save(&requests, &channels)?;
        Ok(id)
    }

    /// A chief asks Atlas for one more worker: a line in its project's `# administration`, and
    /// Atlas wakes (counted in the channel's daily requests). Atlas answers with a hire proposal
    /// the person approves.
    pub(crate) fn visions_ask_for_worker(
        &self,
        sink: &Arc<dyn EventSink>,
        from_channel: &str,
        chief: &str,
        role: &str,
        why: &str,
    ) -> Result<String, String> {
        let (role, why) = (text(role, "role")?, text(why, "reason")?);
        let from = self.channel_get(from_channel).ok_or("Unknown channel")?;
        let hq = self
            .channel_list(&from.project_root)
            .into_iter()
            .find(|channel| channel.name == ADMINISTRATION)
            .ok_or("This project has no # administration yet")?;
        let id = self.record_request(&from, &hq.id, false)?;
        let _ = self.channel_post(
            &hq.id,
            chief,
            ChannelEntry::WorkerAsked {
                vision_id: chief.to_owned(),
                request_id: id.clone(),
                channel_id: from.id.clone(),
                role,
                why,
            },
        );
        self.visions_note(
            "worker_asked",
            json!({"request_id": id, "from_channel": from.id}),
        );
        let admin = super::ADMIN_VISION_ID;
        self.visions_wake(sink, &hq.id, admin, WakeReason::Request(id.clone()));
        Ok(id)
    }

    /// The sending channel reached today's cap: told once a day.
    fn visions_request_cap(&self, channel_id: &str) {
        if self.visions_notice_request_cap(channel_id) {
            self.visions_note("request_cap", json!({"channel_id": channel_id}));
        }
    }

    /// The receiving chief's answer: declining is always its own, and so is accepting, unless
    /// its channel's level leaves the team no decision (Always ask), when the person takes it on.
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
            if super::vision_decisions(&channel, &config.visions).is_empty() {
                return Err(themis_core::prompts::AGENT_CLI_REQUEST_ABOVE_LEVEL.to_owned());
            }
        }
        self.answer_request(sink, channel_id, chief, request_id, accept, note)
    }

    /// The receiving chief (`chief`, leading `channel_id`) sends back what it made for a
    /// request its channel took on: the asking channel shows it and its chief wakes. Once.
    pub(crate) fn visions_send_result(
        &self,
        sink: &Arc<dyn EventSink>,
        channel_id: &str,
        chief: &str,
        request_id: &str,
        text: &str,
    ) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() || text.chars().count() > MAX_TEXT_CHARS {
            return Err(format!(
                "A result is some text of at most {MAX_TEXT_CHARS} characters"
            ));
        }
        let waiting = |request: &Request| {
            request.answered.as_ref().is_some_and(|a| a.accepted) && !request.result_sent
        };
        let request = self
            .request_update(channel_id, request_id, waiting, |request| {
                request.result_sent = true;
            })
            .map_err(|_| {
                "This channel has no request taken on with that id waiting for a result".to_owned()
            })?;
        let entry = ChannelEntry::RequestResult {
            vision_id: chief.to_owned(),
            request_id: request.id.clone(),
            channel_id: request.to_channel.clone(),
            text: text.to_owned(),
        };
        let _ = self.channel_post(&request.from_channel, chief, entry);
        self.visions_note("request_result", json!({"request_id": request.id}));
        if let Some(asker) = self.channel_get(&request.from_channel).map(|c| c.chief) {
            let reason = WakeReason::RequestAnswered(request.id.clone());
            self.visions_wake(sink, &request.from_channel, &asker, reason);
        }
        Ok(())
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
        // The answer is the person's own: no Vision gave it.
        self.answer_request(sink, &to, "", request_id, accept, "")
    }

    /// Changes one request to `channel_id` that `keep` accepts, under the Visions lock, and
    /// returns it as changed.
    fn request_update(
        &self,
        channel_id: &str,
        request_id: &str,
        keep: impl Fn(&Request) -> bool,
        change: impl FnOnce(&mut Request),
    ) -> Result<Request, String> {
        let channels = self.channel_ids();
        let _guard = self.visions_cache();
        let mut requests = self.requests_load()?;
        let request = requests
            .iter_mut()
            .find(|request| request.id == request_id && request.to_channel == channel_id)
            .filter(|request| keep(request))
            .ok_or("This request is no longer open")?;
        change(request);
        let request = request.clone();
        self.requests_save(&requests, &channels)?;
        Ok(request)
    }

    /// Whether `channel`'s team takes another channel's requests on itself: above Always ask
    /// (its level leaves it some decision); at Always ask the person answers.
    /// It takes nothing on while its chief could not run it (paused, or over or unsure of its
    /// budget): the request then stays open for the person.
    fn visions_takes_requests(&self, channel: &super::Channel) -> bool {
        let runnable = !self.paused_in(&channel.id, &channel.chief)
            && self
                .visions_budget_gate(&channel.chief, &channel.id)
                .is_none();
        runnable
            && self
                .runtime_configuration(Some(&channel.project_root), None)
                .is_ok_and(|resolved| {
                    !super::vision_decisions(channel, &resolved.config.visions).is_empty()
                })
    }

    /// Shows an answer to `request` in both its channels (`by` answered; empty for the person).
    fn post_answered(&self, request: &Request, by: &str, accept: bool, note: &str) {
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
            let _ = self.channel_post(here, by, entry);
        }
        self.visions_note(
            "request_answered",
            json!({"request_id": request.id, "accepted": accept, "by_person": by.is_empty()}),
        );
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
        let open = |request: &Request| request.answered.is_none();
        let request = self.request_update(channel_id, request_id, open, |request| {
            request.answered = Some(Answered {
                accepted: accept,
                at_ms: now_ms(),
            });
        })?;
        self.post_answered(&request, by, accept, note);
        let answered = || WakeReason::RequestAnswered(request.id.clone());
        if let Some(asker) = self.channel_get(&request.from_channel).map(|c| c.chief) {
            self.visions_wake(sink, &request.from_channel, &asker, answered());
        }
        // The person took the work on for their channel: its chief wakes to do it.
        if by.is_empty() && accept {
            if let Some(chief) = self.channel_get(&request.to_channel).map(|c| c.chief) {
                self.visions_wake(sink, &request.to_channel, &chief, answered());
            }
        }
        Ok(())
    }
}
