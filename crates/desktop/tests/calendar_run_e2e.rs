//! A failed provider run must retain the calendar recurrence, with review history.
use chrono::{Datelike, Utc};
use std::sync::Arc;
use themis_desktop::{
    sink::{ChannelSink, TestEvent},
    state::AppState,
    types::{AutomationInput, AutomationRepeat, AutomationSchedule, ProviderKind, ThreadEvent},
};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

#[tokio::test]
async fn failed_calendar_run_advances_wall_clock_schedule_and_persists_review() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let provider = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string("fixture refused"))
        .mount(&provider)
        .await;
    state
        .set_secret("go".into(), "fixture-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let schedule = AutomationSchedule {
        repeat: AutomationRepeat::Weekdays,
        time: "09:00".into(),
        timezone: "Europe/Berlin".into(),
        weekday: 0,
    };
    let created = state
        .create_automation(AutomationInput {
            name: "Calendar failure".into(),
            project_root: project.path().to_str().unwrap().into(),
            target_thread_id: None,
            provider: ProviderKind::Go,
            model: "fixture".into(),
            reasoning_effort: None,
            skill_ids: vec![],
            interval_mins: 60,
            schedule: Some(schedule.clone()),
            task: "Review changes".into(),
            enabled: true,
        })
        .await
        .unwrap();
    state
        .set_automation_next_run_for_test(&created.id, "2001-01-01T00:00:00Z".into())
        .await
        .unwrap();
    let (sink, mut rx) = ChannelSink::channel();
    state
        .run_automation_now(Arc::new(sink), created.id.clone())
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while let Some(event) = rx.recv().await {
            if let TestEvent::Thread(envelope) = event {
                if matches!(envelope.event, ThreadEvent::Failed { .. }) {
                    return;
                }
                assert!(!matches!(envelope.event, ThreadEvent::Finished { .. }));
            }
        }
        panic!("missing failed event");
    })
    .await
    .unwrap();
    let advanced = state.list_automations().await.remove(0);
    assert_eq!(advanced.schedule, Some(schedule));
    assert_eq!(advanced.run_count, 1);
    let next = chrono::DateTime::parse_from_rfc3339(&advanced.next_run_at).unwrap();
    assert!(next > Utc::now());
    let local = next.with_timezone(&chrono_tz::Europe::Berlin);
    assert_eq!(local.format("%H:%M").to_string(), "09:00");
    assert!(local.weekday().num_days_from_monday() < 5);
    let reviews = state.list_review_items(None).await;
    assert_eq!(reviews.len(), 1);
    assert!(reviews[0].summary.starts_with("failed:"));
    drop(state);
    let rebooted = AppState::new_for_test(data.path().join("settings.json"));
    assert_eq!(rebooted.list_automations().await, vec![advanced]);
    assert_eq!(rebooted.list_review_items(None).await, reviews);
}
