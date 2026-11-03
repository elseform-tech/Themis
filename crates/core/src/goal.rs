//! Goal mode: the text the runtime sends between goal turns, and how a failed run is
//! classified.
//!
//! A goal is the person's objective for a chat. The runtime starts goal turn after goal turn
//! until the chat's own model marks it complete or blocked with `update_goal`, or the person
//! pauses it. There is no separate check: the model audits its own work, and a complete goal
//! carries the evidence it named for each requirement.

use serde::{Deserialize, Serialize};

/// Marks a message the runtime sends to continue a goal; never the user's words.
pub const GOAL_TURN_MARKER: &str = "[Goal — runtime, not from the user]";

/// Longest goal sentence, in characters.
pub const MAX_GOAL_CHARS: usize = 2000;

/// Collapses whitespace, drops control characters and keeps at most `max` characters.
pub fn one_line(text: &str, max: usize) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

/// A goal sentence as stored: one line, bounded, never empty after trimming.
pub fn normalize_goal(text: &str) -> Option<String> {
    let goal = one_line(text, MAX_GOAL_CHARS);
    (!goal.is_empty()).then_some(goal)
}

/// The sentence after a leading `/goal` command, if the message is one
/// (`Some("")` for a bare `/goal`). `/goals` or `/goalie` are not the command.
pub fn parse_goal_command(text: &str) -> Option<String> {
    let rest = text.trim_start().strip_prefix("/goal")?;
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    Some(normalize_goal(rest).unwrap_or_default())
}

/// What every run in a chat with a running goal is told about it, after the
/// user's message: the user's request comes first, the goal continues after.
pub fn goal_context(goal: &str) -> String {
    format!(
        "\n\n{}",
        crate::prompts::fill(crate::prompts::GOAL_CONTEXT, &[("goal", goal)])
    )
}

/// What a message is told when the chat's goal is paused, so old goal turns don't read as work
/// to resume.
pub fn paused_context(goal: &str) -> String {
    format!(
        "\n\n{}",
        crate::prompts::fill(crate::prompts::GOAL_CONTEXT_PAUSED, &[("goal", goal)])
    )
}

/// What a message is told when the chat once had a goal and none is running.
pub fn over_context() -> String {
    format!("\n\n{}", crate::prompts::GOAL_CONTEXT_OVER)
}

/// The message that starts goal turn `turn`. `note` says why it continues
/// (an interruption or a failure), empty when there is nothing to say.
pub fn turn_message(turn: u32, goal: &str, note: &str) -> String {
    crate::prompts::fill(
        crate::prompts::GOAL_TURN,
        &[
            ("goal_turn_marker", GOAL_TURN_MARKER),
            ("turn", &turn.to_string()),
            ("goal", goal),
            ("note", note),
        ],
    )
}

/// The turn number in a goal-turn message, if `text` is one.
pub fn turn_number(text: &str) -> Option<u32> {
    let rest = text.strip_prefix(GOAL_TURN_MARKER)?.trim_start();
    let digits: String = rest
        .strip_prefix("Turn ")?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// Most evidence entries one `update_goal` call may carry.
pub const MAX_EVIDENCE: usize = 40;
/// Longest requirement and proof of one evidence entry, in characters.
pub const MAX_REQUIREMENT_CHARS: usize = 300;
pub const MAX_PROOF_CHARS: usize = 600;
/// Longest reason a blocked goal gives, in characters.
pub const MAX_BLOCKED_REASON_CHARS: usize = 400;

/// One requirement of a complete goal and what proves it, as the model named them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub requirement: String,
    pub proof: String,
}

/// What an `update_goal` call asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalUpdate {
    /// The objective is achieved; one entry per requirement.
    Complete(Vec<Evidence>),
    /// The same blocker held for three goal turns running; why, in a sentence.
    Blocked(String),
    /// The person asked to pause.
    Paused,
}

/// Reads an `update_goal` call's arguments. A refusal says why, so the model can fix the call:
/// `complete` needs a non-empty `evidence` list whose every entry names a requirement and its
/// proof; `blocked` needs a `reason`. Text is bounded and kept on one line.
pub fn parse_update(args: &serde_json::Value) -> Result<GoalUpdate, String> {
    match args["status"].as_str() {
        Some("complete") => {
            let Some(entries) = args["evidence"].as_array().filter(|list| !list.is_empty()) else {
                return Err("update_goal: `complete` needs `evidence`, a list with one entry per requirement of the objective, each {\"requirement\", \"proof\"} where proof is what shows it now (a command and its result, a file, a test, a page check). Audit every requirement first; if one is not proven, keep working instead.".to_owned());
            };
            if entries.len() > MAX_EVIDENCE {
                return Err(format!(
                    "update_goal: at most {MAX_EVIDENCE} evidence entries; group related requirements."
                ));
            }
            let mut evidence = Vec::with_capacity(entries.len());
            for (index, entry) in entries.iter().enumerate() {
                let requirement = one_line(
                    entry["requirement"].as_str().unwrap_or_default(),
                    MAX_REQUIREMENT_CHARS,
                );
                let proof = one_line(entry["proof"].as_str().unwrap_or_default(), MAX_PROOF_CHARS);
                if requirement.is_empty() || proof.is_empty() {
                    return Err(format!(
                        "update_goal: evidence entry {} needs both `requirement` and `proof`. Name what was required and what current evidence proves it; a requirement you cannot prove is not done, so keep working on it instead.",
                        index + 1
                    ));
                }
                evidence.push(Evidence { requirement, proof });
            }
            Ok(GoalUpdate::Complete(evidence))
        }
        Some("blocked") => {
            let reason = one_line(
                args["reason"].as_str().unwrap_or_default(),
                MAX_BLOCKED_REASON_CHARS,
            );
            if reason.is_empty() {
                return Err("update_goal: `blocked` needs `reason`: the blocker, in one sentence the person can act on.".to_owned());
            }
            Ok(GoalUpdate::Blocked(reason))
        }
        Some("paused") => Ok(GoalUpdate::Paused),
        _ => Err("update_goal: `status` must be complete, blocked or paused.".to_owned()),
    }
}

/// What a failed run means for a goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The user pressed Stop: the turn ends, the goal continues.
    Stopped,
    /// The provider says the model quota or credits are used up.
    Quota,
    /// A rate limit, timeout or server error: retry after a wait.
    Transient,
    /// Anything else: the next turn is told the last one failed.
    Other,
}

/// Classifies a run's failure text. A rate limit counts as quota only when the
/// provider says quota, credits, billing or usage limit, or asks to wait more
/// than ten minutes (`Retry-After`).
pub fn classify_failure(error: &str) -> Failure {
    if error.starts_with(crate::runtime::STOPPED_BY_YOU) {
        return Failure::Stopped;
    }
    let lower = error.to_ascii_lowercase();
    let long_wait = lower
        .split("retry-after: ")
        .nth(1)
        .and_then(|rest| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<u64>()
                .ok()
        })
        .is_some_and(|seconds| seconds > 600);
    if long_wait {
        return Failure::Quota;
    }
    let quota_words = [
        "quota",
        "credit",
        "billing",
        "usage limit",
        "insufficient",
        "exceeded your",
        "payment",
    ];
    if quota_words.iter().any(|word| lower.contains(word)) {
        return Failure::Quota;
    }
    let transient = [
        "429",
        "rate limit",
        "ratelimit",
        "timed out",
        "timeout",
        "http 5",
        "connection",
        "temporarily",
        "overloaded",
        "unavailable",
        // A request that never reached the provider, and a model that replied with nothing.
        "error sending request",
        "stream request failed",
        "no public answer",
    ];
    if transient.iter().any(|word| lower.contains(word)) {
        return Failure::Transient;
    }
    Failure::Other
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn complete_needs_evidence_for_every_requirement() {
        let refused = parse_update(&serde_json::json!({"status": "complete"})).unwrap_err();
        assert!(refused.contains("needs `evidence`"), "{refused}");
        let empty = parse_update(&serde_json::json!({"status": "complete", "evidence": []}));
        assert!(empty.unwrap_err().contains("needs `evidence`"));
        let blank = parse_update(&serde_json::json!({"status": "complete", "evidence": [
            {"requirement": "tests pass", "proof": "cargo test: 12 passed"},
            {"requirement": "README updated", "proof": "  "}
        ]}))
        .unwrap_err();
        assert!(blank.contains("entry 2"), "{blank}");
        assert_eq!(
            parse_update(&serde_json::json!({"status": "complete", "evidence": [
                {"requirement": "tests\npass", "proof": "cargo test: 12 passed"}
            ]})),
            Ok(GoalUpdate::Complete(vec![Evidence {
                requirement: "tests pass".into(),
                proof: "cargo test: 12 passed".into()
            }]))
        );
    }

    #[test]
    fn blocked_needs_a_reason_and_paused_needs_nothing() {
        assert!(parse_update(&serde_json::json!({"status": "blocked"})).is_err());
        assert_eq!(
            parse_update(&serde_json::json!({"status": "blocked", "reason": "No API key."})),
            Ok(GoalUpdate::Blocked("No API key.".into()))
        );
        assert_eq!(
            parse_update(&serde_json::json!({"status": "paused"})),
            Ok(GoalUpdate::Paused)
        );
        assert!(parse_update(&serde_json::json!({"status": "done"})).is_err());
        assert!(parse_update(&serde_json::json!(7)).is_err());
    }

    #[test]
    fn goal_command_is_recognized_only_at_the_start() {
        assert_eq!(
            parse_goal_command("  /goal  speed up\n p95 "),
            Some("speed up p95".into())
        );
        assert_eq!(parse_goal_command("/goal"), Some(String::new()));
        assert_eq!(parse_goal_command("/goals list"), None);
        assert_eq!(parse_goal_command("my /goal is speed"), None);
    }

    #[test]
    fn turn_messages_carry_their_number() {
        let text = turn_message(24, "speed up p95", "p95 still 238 ms");
        assert_eq!(turn_number(&text), Some(24));
        assert_eq!(turn_number("Turn 3 of mine"), None);
    }

    #[test]
    fn failures_are_classified() {
        assert_eq!(
            classify_failure("Stopped by you. Completed actions remain available for review."),
            Failure::Stopped
        );
        assert_eq!(
            classify_failure("LLM request failed: OpenCode Go request failed with HTTP 429: monthly usage limit reached"),
            Failure::Quota
        );
        assert_eq!(
            classify_failure("LLM request failed: OpenCode Go request failed with HTTP 429"),
            Failure::Transient
        );
        assert_eq!(
            classify_failure("LLM request failed: request timed out"),
            Failure::Transient
        );
        assert_eq!(
            classify_failure("Rate Limit Error (429): slow down. Retry-After: 3600s"),
            Failure::Quota
        );
        assert_eq!(
            classify_failure("Rate Limit Error (429): slow down. Retry-After: 30s"),
            Failure::Transient
        );
        // Both seen in a live goal: they clear by themselves.
        assert_eq!(
            classify_failure("LLM request failed: HTTP Error: stream request failed: error sending request for url (https://opencode.ai/inference/go/openai/v1/chat/completions)"),
            Failure::Transient
        );
        assert_eq!(
            classify_failure("LLM request failed: Generic Error : Provider returned no public answer or tool calls"),
            Failure::Transient
        );
        assert_eq!(classify_failure("agent run panicked: boom"), Failure::Other);
    }

    proptest! {
        /// `update_goal` arguments come from a model: whatever they hold, an accepted update
        /// stays bounded and every evidence entry names a requirement and its proof.
        #[test]
        fn any_update_stays_bounded(status in "(complete|blocked|paused|.{0,8})", text in ".{0,900}", count in 0usize..60) {
            let args = serde_json::json!({"status": status, "reason": text,
                "evidence": vec![serde_json::json!({"requirement": text, "proof": text}); count]});
            match parse_update(&args) {
                Ok(GoalUpdate::Complete(evidence)) => {
                    prop_assert!(!evidence.is_empty() && evidence.len() <= MAX_EVIDENCE);
                    for entry in &evidence {
                        prop_assert!(!entry.requirement.is_empty() && entry.requirement.chars().count() <= MAX_REQUIREMENT_CHARS);
                        prop_assert!(!entry.proof.is_empty() && entry.proof.chars().count() <= MAX_PROOF_CHARS);
                        prop_assert!(!entry.proof.chars().any(char::is_control));
                    }
                }
                Ok(GoalUpdate::Blocked(reason)) => prop_assert!(!reason.is_empty() && reason.chars().count() <= MAX_BLOCKED_REASON_CHARS),
                Ok(GoalUpdate::Paused) => prop_assert_eq!(status, "paused"),
                Err(why) => prop_assert!(why.starts_with("update_goal:")),
            }
        }

        #[test]
        fn goal_commands_never_panic(text in ".{0,200}") {
            if let Some(goal) = parse_goal_command(&text) {
                prop_assert!(goal.chars().count() <= MAX_GOAL_CHARS);
                prop_assert!(!goal.contains('\n'));
            }
            let _ = classify_failure(&text);
            let _ = turn_number(&text);
        }
    }
}
