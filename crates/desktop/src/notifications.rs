//! Desktop notifications when a goal is reached or pauses while you are away.
//!
//! The app sends them, not the headless server: only the app knows whether its
//! window is focused, and the system attributes notifications to its bundle.
//! Which goal changes notify is decided here, in plain tested code; delivery
//! uses macOS User Notifications and is unavailable elsewhere (and in an
//! unbundled development binary). Each attempt is logged by the server as
//! metadata only: chat id, kind and outcome, never the chat's title or text.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::types::{Goal, GoalStatus, THREAD_EVENT_NAME};

/// What a goal notification reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalNotice {
    /// The goal check found the goal reached.
    Reached,
    /// Paused: no progress over several turns, repeated failures, or unreachable.
    Stuck,
    /// Paused until you answer a question.
    Waiting,
    /// Paused because the model quota is used up; it retries on its own.
    Quota,
}

/// How a notification attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationOutcome {
    Sent,
    /// Notifications are off for Themis in the system settings.
    Denied,
    /// The system refused or failed to deliver it.
    Failed,
    /// This platform or build cannot send notifications.
    Unavailable,
}

/// The metadata the server logs for one attempt (`record_notification`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationRecord {
    pub thread_id: String,
    pub kind: GoalNotice,
    pub outcome: NotificationOutcome,
}

/// Chats a [`GoalWatch`] remembers before it starts over (bounds its memory).
const MAX_WATCHED_CHATS: usize = 1024;

#[derive(Debug, Clone, Copy)]
struct Seen {
    status: GoalStatus,
    started_at: i64,
    /// A quota pause was already announced; its 15-minute retries stay quiet.
    quota_noticed: bool,
}

/// Remembers each chat's last goal status, so only real changes notify.
#[derive(Debug, Default)]
pub struct GoalWatch {
    chats: HashMap<String, Seen>,
}

impl GoalWatch {
    /// Records the chat's goal as the server announced it and returns what to
    /// notify, if anything. A goal reached, stuck, or waiting for you notifies
    /// once when it gets there; a quota pause notifies once until the goal
    /// leaves the retry cycle. Running, a pause you made, and a removed goal
    /// never notify.
    pub fn observe(&mut self, thread_id: &str, goal: Option<&Goal>) -> Option<GoalNotice> {
        let Some(goal) = goal else {
            self.chats.remove(thread_id);
            return None;
        };
        // A new goal (another start time) starts with a clean slate.
        let previous = self
            .chats
            .get(thread_id)
            .copied()
            .filter(|seen| seen.started_at == goal.started_at);
        let changed = previous.is_none_or(|seen| seen.status != goal.status);
        let quota_noticed = previous.is_some_and(|seen| seen.quota_noticed);
        let notice = match goal.status {
            _ if !changed => None,
            GoalStatus::Reached => Some(GoalNotice::Reached),
            GoalStatus::Stuck => Some(GoalNotice::Stuck),
            GoalStatus::Waiting => Some(GoalNotice::Waiting),
            GoalStatus::Quota if !quota_noticed => Some(GoalNotice::Quota),
            GoalStatus::Quota | GoalStatus::Running | GoalStatus::Paused => None,
        };
        let quota_noticed = match goal.status {
            GoalStatus::Quota => true,
            GoalStatus::Running => quota_noticed,
            _ => false,
        };
        if self.chats.len() >= MAX_WATCHED_CHATS && !self.chats.contains_key(thread_id) {
            self.chats.clear();
        }
        self.chats.insert(
            thread_id.to_owned(),
            Seen {
                status: goal.status,
                started_at: goal.started_at,
                quota_noticed,
            },
        );
        notice
    }
}

/// The chat id and goal of a `goal_changed` frame from the server's event
/// stream (`{name, payload}`); `None` for any other frame.
#[must_use]
pub fn goal_change(frame: &Value) -> Option<(String, Option<Goal>)> {
    if frame.get("name")?.as_str()? != THREAD_EVENT_NAME {
        return None;
    }
    let payload = frame.get("payload")?;
    let event = payload.get("event")?;
    if event.get("kind")?.as_str()? != "goal_changed" {
        return None;
    }
    let thread_id = payload.get("thread_id")?.as_str()?.to_owned();
    let goal = match event.get("goal") {
        None | Some(Value::Null) => None,
        Some(goal) => Some(serde_json::from_value(goal.clone()).ok()?),
    };
    Some((thread_id, goal))
}

/// The notification's title (the chat) and body (what happened).
#[must_use]
pub fn notice_text(kind: GoalNotice, chat_title: &str, reason: Option<&str>) -> (String, String) {
    let title = themis_core::goal::one_line(chat_title, 80);
    let title = if title.is_empty() {
        "Themis".to_owned()
    } else {
        title
    };
    let reason = reason.map(|reason| themis_core::goal::one_line(reason, 160));
    let with_reason = |lead: &str| match reason.as_deref() {
        Some(reason) if !reason.is_empty() => format!("{lead}: {reason}"),
        _ => format!("{lead}."),
    };
    let body = match kind {
        GoalNotice::Reached => with_reason("Goal reached"),
        GoalNotice::Stuck => with_reason("Goal paused"),
        GoalNotice::Waiting => "Goal paused until you answer.".to_owned(),
        GoalNotice::Quota => {
            "Goal paused: the model quota is used up. It tries again every 15 minutes.".to_owned()
        }
    };
    (title, body)
}

/// The Tauri event the web UI listens on to open a notification's chat.
pub const NOTIFICATION_OPENED_NAME: &str = "notification-opened";

/// Whether Themis may show notifications (`notification_permission`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Granted,
    Denied,
    /// Not asked yet.
    Undetermined,
    /// This platform or build cannot send notifications.
    Unavailable,
}

/// Reports, and with `request` asks for, permission to notify. Settings asks
/// when you turn the switch on; a goal asks the first time it needs to.
#[tauri::command]
pub async fn notification_permission(request: bool) -> Permission {
    native::permission(request).await
}

/// Watches the server's events for goal changes and notifies on the ones
/// [`GoalWatch`] picks, when the switch is on and the main window is not focused.
pub fn watch_event(
    app: &tauri::AppHandle,
    client: &crate::server::Client,
    watch: &mut GoalWatch,
    frame: &Value,
) {
    let Some((thread_id, goal)) = goal_change(frame) else {
        return;
    };
    let Some(kind) = watch.observe(&thread_id, goal.as_ref()) else {
        return;
    };
    let reason = goal.and_then(|goal| goal.reason);
    let app = app.clone();
    let client = client.clone();
    tauri::async_runtime::spawn(async move {
        deliver(&app, &client, thread_id, kind, reason).await;
    });
}

async fn deliver(
    app: &tauri::AppHandle,
    client: &crate::server::Client,
    thread_id: String,
    kind: GoalNotice,
    reason: Option<String>,
) {
    use tauri::Manager;
    let enabled = client
        .call("get_settings", Value::Null)
        .await
        .ok()
        .and_then(|settings| settings.get("goal_notifications")?.as_bool())
        .unwrap_or(true);
    let focused = app
        .get_webview_window("main")
        .and_then(|window| window.is_focused().ok())
        .unwrap_or(false);
    if !enabled || focused {
        return;
    }
    let chat_title = client
        .call("get_thread", json!({ "threadId": thread_id }))
        .await
        .ok()
        .and_then(|thread| thread.get("title")?.as_str().map(str::to_owned))
        .unwrap_or_default();
    let (title, body) = notice_text(kind, &chat_title, reason.as_deref());
    let outcome = native::send(&thread_id, &title, &body).await;
    let record = NotificationRecord {
        thread_id,
        kind,
        outcome,
    };
    let _ = client
        .call("record_notification", json!({ "record": record }))
        .await;
}

/// Brings the main window forward and asks the web UI to open the chat.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn open_chat(app: &tauri::AppHandle, thread_id: String) {
    use tauri::{Emitter, Manager};
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    if !thread_id.is_empty() {
        let _ = app.emit(NOTIFICATION_OPENED_NAME, thread_id);
    }
}

/// Sets up delivery and click handling. Call once from the app's setup.
pub fn install(app: &tauri::AppHandle) {
    native::install(app);
}

#[cfg(target_os = "macos")]
mod native {
    //! macOS User Notifications. The framework requires a real app bundle and
    //! raises an exception without one, so every entry point checks first.

    use std::ptr::NonNull;
    use std::sync::{Mutex, OnceLock};

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, NSObject, ProtocolObject};
    use objc2::{define_class, msg_send, AnyThread};
    use objc2_foundation::{NSBundle, NSError, NSObjectProtocol, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent,
        UNNotification, UNNotificationPresentationOptions, UNNotificationRequest,
        UNNotificationResponse, UNNotificationSettings, UNUserNotificationCenter,
        UNUserNotificationCenterDelegate,
    };

    use super::{NotificationOutcome, Permission};

    static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

    fn center() -> Option<Retained<UNUserNotificationCenter>> {
        let bundled = NSBundle::mainBundle()
            .bundlePath()
            .to_string()
            .ends_with(".app");
        bundled.then(UNUserNotificationCenter::currentNotificationCenter)
    }

    /// A one-shot reply from a completion handler the framework may call on any thread.
    fn reply<T: Send + 'static>() -> (
        impl Fn(T) + Send + Sync + 'static,
        tokio::sync::oneshot::Receiver<T>,
    ) {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let sender = Mutex::new(Some(sender));
        let send = move |value: T| {
            if let Some(sender) = sender.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let _ = sender.send(value);
            }
        };
        (send, receiver)
    }

    fn status_request() -> Option<tokio::sync::oneshot::Receiver<UNAuthorizationStatus>> {
        let center = center()?;
        let (send, receiver) = reply();
        let block = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
            // SAFETY: the framework passes a valid settings object for the call.
            send(unsafe { settings.as_ref() }.authorizationStatus());
        });
        center.getNotificationSettingsWithCompletionHandler(&block);
        Some(receiver)
    }

    fn authorization_request() -> Option<tokio::sync::oneshot::Receiver<bool>> {
        let center = center()?;
        let (send, receiver) = reply();
        let block =
            RcBlock::new(move |granted: Bool, _error: *mut NSError| send(granted.as_bool()));
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &block,
        );
        Some(receiver)
    }

    pub(super) async fn permission(request: bool) -> Permission {
        let Some(status) = status_request() else {
            return Permission::Unavailable;
        };
        match status.await {
            Ok(UNAuthorizationStatus::NotDetermined) if request => match authorization_request() {
                Some(granted) => {
                    if granted.await.unwrap_or(false) {
                        Permission::Granted
                    } else {
                        Permission::Denied
                    }
                }
                None => Permission::Unavailable,
            },
            Ok(UNAuthorizationStatus::NotDetermined) => Permission::Undetermined,
            Ok(UNAuthorizationStatus::Denied) => Permission::Denied,
            Ok(_) => Permission::Granted,
            Err(_) => Permission::Unavailable,
        }
    }

    fn post(
        thread_id: &str,
        title: &str,
        body: &str,
    ) -> Option<tokio::sync::oneshot::Receiver<bool>> {
        let center = center()?;
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
        // Groups a chat's notifications and tells a click which chat to open.
        content.setThreadIdentifier(&NSString::from_str(thread_id));
        let identifier = NSString::from_str(&format!("goal-{thread_id}"));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &identifier,
            &content,
            None,
        );
        let (send, receiver) = reply();
        let block = RcBlock::new(move |error: *mut NSError| send(error.is_null()));
        center.addNotificationRequest_withCompletionHandler(&request, Some(&block));
        Some(receiver)
    }

    pub(super) async fn send(thread_id: &str, title: &str, body: &str) -> NotificationOutcome {
        match permission(true).await {
            Permission::Granted => {}
            Permission::Denied | Permission::Undetermined => return NotificationOutcome::Denied,
            Permission::Unavailable => return NotificationOutcome::Unavailable,
        }
        let Some(delivered) = post(thread_id, title, body) else {
            return NotificationOutcome::Unavailable;
        };
        if delivered.await.unwrap_or(false) {
            NotificationOutcome::Sent
        } else {
            NotificationOutcome::Failed
        }
    }

    define_class!(
        /// Shows banners while another Themis window is in front, and opens
        /// the chat when you click one.
        #[unsafe(super(NSObject))]
        #[name = "ThemisNotificationDelegate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                handler.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List,));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                handler: &block2::DynBlock<dyn Fn()>,
            ) {
                let thread_id = response
                    .notification()
                    .request()
                    .content()
                    .threadIdentifier()
                    .to_string();
                if let Some(app) = APP.get() {
                    super::open_chat(app, thread_id);
                }
                handler.call(());
            }
        }
    );

    pub(super) fn install(app: &tauri::AppHandle) {
        let Some(center) = center() else {
            return;
        };
        if APP.set(app.clone()).is_err() {
            return;
        }
        // SAFETY: `init` on a freshly allocated NSObject subclass without ivars.
        let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(), init] };
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        // The center holds its delegate weakly; this one lives as long as the app.
        std::mem::forget(delegate);
    }
}

#[cfg(not(target_os = "macos"))]
mod native {
    use super::{NotificationOutcome, Permission};

    #[allow(clippy::unused_async)]
    pub(super) async fn permission(_request: bool) -> Permission {
        Permission::Unavailable
    }

    #[allow(clippy::unused_async)]
    pub(super) async fn send(_thread_id: &str, _title: &str, _body: &str) -> NotificationOutcome {
        NotificationOutcome::Unavailable
    }

    pub(super) fn install(_app: &tauri::AppHandle) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal(status: GoalStatus) -> Goal {
        serde_json::from_value(json!({
            "text": "ship it",
            "status": status,
            "started_at": 1,
            "reason": "All tests pass.",
        }))
        .unwrap()
    }

    fn statuses(watch: &mut GoalWatch, steps: &[GoalStatus]) -> Vec<Option<GoalNotice>> {
        steps
            .iter()
            .map(|status| watch.observe("chat", Some(&goal(*status))))
            .collect()
    }

    #[test]
    fn reached_stuck_and_waiting_notify_once_when_the_goal_gets_there() {
        use GoalStatus::{Reached, Running, Stuck, Waiting};
        let mut watch = GoalWatch::default();
        assert_eq!(
            statuses(
                &mut watch,
                &[
                    Running, Running, Waiting, Waiting, Running, Stuck, Stuck, Running, Reached,
                    Reached
                ]
            ),
            [
                None,
                None,
                Some(GoalNotice::Waiting),
                None,
                None,
                Some(GoalNotice::Stuck),
                None,
                None,
                Some(GoalNotice::Reached),
                None,
            ]
        );
    }

    #[test]
    fn running_your_own_pause_and_a_removed_goal_stay_quiet() {
        use GoalStatus::{Paused, Running};
        let mut watch = GoalWatch::default();
        assert_eq!(
            statuses(&mut watch, &[Running, Paused, Running, Paused]),
            [None; 4]
        );
        assert_eq!(watch.observe("chat", None), None);
        assert!(watch.chats.is_empty());
    }

    #[test]
    fn a_quota_pause_notifies_once_across_its_retries() {
        use GoalStatus::{Paused, Quota, Running};
        let mut watch = GoalWatch::default();
        // Each 15-minute retry runs a turn and pauses again: one notification.
        assert_eq!(
            statuses(
                &mut watch,
                &[Running, Quota, Running, Quota, Running, Quota]
            ),
            [None, Some(GoalNotice::Quota), None, None, None, None]
        );
        // After you pause and resume, a new quota pause is news again.
        assert_eq!(
            statuses(&mut watch, &[Paused, Running, Quota]),
            [None, None, Some(GoalNotice::Quota)]
        );
    }

    #[test]
    fn a_new_goal_in_the_same_chat_starts_over() {
        let mut watch = GoalWatch::default();
        assert_eq!(
            watch.observe("chat", Some(&goal(GoalStatus::Reached))),
            Some(GoalNotice::Reached)
        );
        let mut next = goal(GoalStatus::Reached);
        next.started_at = 2;
        assert_eq!(
            watch.observe("chat", Some(&next)),
            Some(GoalNotice::Reached)
        );
        // Chats are tracked separately.
        assert_eq!(
            watch.observe("other", Some(&goal(GoalStatus::Reached))),
            Some(GoalNotice::Reached)
        );
    }

    #[test]
    fn the_watch_stays_bounded() {
        let mut watch = GoalWatch::default();
        for index in 0..=MAX_WATCHED_CHATS {
            watch.observe(&format!("chat-{index}"), Some(&goal(GoalStatus::Running)));
        }
        assert!(watch.chats.len() <= MAX_WATCHED_CHATS);
    }

    #[test]
    fn only_goal_changed_frames_are_read() {
        let frame = |event: Value| json!({"name": THREAD_EVENT_NAME, "payload": {"thread_id": "t1", "run_id": "", "event": event}});
        let (thread_id, read) = goal_change(&frame(
            json!({"kind": "goal_changed", "goal": goal(GoalStatus::Stuck)}),
        ))
        .unwrap();
        assert_eq!(thread_id, "t1");
        assert_eq!(read.unwrap().status, GoalStatus::Stuck);
        assert_eq!(
            goal_change(&frame(json!({"kind": "goal_changed", "goal": null}))),
            Some(("t1".to_owned(), None))
        );
        assert!(goal_change(&frame(json!({"kind": "finished"}))).is_none());
        assert!(goal_change(&frame(json!({"kind": "goal_changed", "goal": {"bad": 1}}))).is_none());
        assert!(goal_change(&json!({"name": "review-item-added", "payload": {}})).is_none());
    }

    #[test]
    fn the_text_names_the_chat_and_what_happened() {
        assert_eq!(
            notice_text(
                GoalNotice::Reached,
                "Fix the build",
                Some("All tests pass.")
            ),
            (
                "Fix the build".to_owned(),
                "Goal reached: All tests pass.".to_owned()
            )
        );
        assert_eq!(
            notice_text(GoalNotice::Stuck, "", None),
            ("Themis".to_owned(), "Goal paused.".to_owned())
        );
        let (_, body) = notice_text(GoalNotice::Stuck, "x", Some(&"word ".repeat(100)));
        assert!(body.chars().count() <= 200, "{body}");
        assert_eq!(
            notice_text(GoalNotice::Waiting, "x", Some("Which port?")).1,
            "Goal paused until you answer."
        );
    }

    #[test]
    fn records_are_metadata_only() {
        let record: NotificationRecord = serde_json::from_value(
            json!({"thread_id": "t1", "kind": "quota", "outcome": "denied"}),
        )
        .unwrap();
        assert_eq!(record.kind, GoalNotice::Quota);
        assert_eq!(record.outcome, NotificationOutcome::Denied);
        assert!(serde_json::from_value::<NotificationRecord>(
            json!({"thread_id": "t1", "kind": "quota", "outcome": "sent", "title": "secret"})
        )
        .is_err());
    }
}
