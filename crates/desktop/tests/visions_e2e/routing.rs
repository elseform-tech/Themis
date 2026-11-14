//! Teams through the real app server: questions in a worker's chat reach the person under the
//! channel's chief, even when no agent can take them.

use super::*;

/// The chief of the channel `vision_and_channel` made (its first member).
fn chief_of(fx: &Fixture, channel: &str) -> String {
    let listed = fx.call_ok("list_channels", json!({"projectRoot": fx.project}));
    let found = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == channel)
        .unwrap();
    found["chief"].as_str().unwrap().to_owned()
}

fn passed_on(fx: &Fixture, channel: &str) -> Vec<Value> {
    fx.entries(channel)
        .into_iter()
        .filter(|entry| entry["type"] == "passed_on")
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_question_no_agent_may_answer_reaches_the_person_anyway() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let provider = scripted_provider().await;
    fx.use_provider(&provider).await;
    let (worker, channel) = fx.vision_and_channel("Rook", "release");
    // The test plays Rook's run, so no Vision run can wake to take the question.
    let (_, token) = fx.vision_run(&worker, &channel).await;
    let (work, question) = fx.asked_question(&token).await;
    let chief = chief_of(&fx, &channel);
    wait_until(|| async { !passed_on(&fx, &channel).is_empty() }).await;
    let passed = passed_on(&fx, &channel);
    assert_eq!(passed[0]["vision_id"], chief.as_str(), "{passed:?}");
    assert_eq!(passed[0]["thread_id"], work.as_str());
    assert_eq!(passed[0]["question_id"], question.as_str());
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workers_passed_question_shows_once_under_its_chief() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let provider = scripted_provider().await;
    fx.use_provider(&provider).await;
    let (worker, channel) = fx.vision_and_channel("Rook", "release");
    let (_, token) = fx.vision_run(&worker, &channel).await;
    let (work, question) = fx.asked_question(&token).await;
    wait_until(|| async { !passed_on(&fx, &channel).is_empty() }).await;
    // The worker passes it on too: the person still sees it once, under the chief.
    fx.agent_call_ok(
        &token,
        "pass_question",
        json!({"threadId": work, "questionId": question, "decision": "scope"}),
    );
    let all = passed_on(&fx, &channel);
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0]["vision_id"], chief_of(&fx, &channel).as_str());
    fx.shut_down().await;
}

/// Wren's chief (`vision_and_channel`'s lead) when the person writes: it finds Wren on its team
/// and gives it the work, then says so.
pub(crate) fn lead_tasks_wren(messages: &[Value]) -> ResponseTemplate {
    let last = messages
        .iter()
        .rposition(|m| m["role"] == "user")
        .unwrap_or(0);
    let results: Vec<&str> = messages[last..]
        .iter()
        .filter(|m| m["role"] == "tool")
        .filter_map(|m| m["content"].as_str())
        .collect();
    let call = |method: &str, args: Value| {
        let arguments = json!({"command": "themis", "args": ["call", method, args.to_string()]});
        let call = json!({"id": format!("lead_{}", results.len()), "type": "function",
            "function": {"name": "shell", "arguments": arguments.to_string()}});
        ResponseTemplate::new(200).set_body_json(json!({"choices": [{"message":
            {"role": "assistant", "content": null, "tool_calls": [call]},
            "finish_reason": "tool_calls"}]}))
    };
    match results.as_slice() {
        [] => call("list_team", json!({})),
        [team] => {
            let id = result_object(team).and_then(|team| team[0]["id"].as_str().map(str::to_owned));
            call(
                "assign_task",
                json!({"workerId": id, "task": "Please add a greeting"}),
            )
        }
        _ => ResponseTemplate::new(200).set_body_json(json!({"choices": [{"message":
            {"role": "assistant", "content": "Asked Wren to add the greeting."},
            "finish_reason": "stop"}]})),
    }
}
