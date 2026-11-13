//! Which saved history items the model sees as conversation turns.

use super::*;

pub(super) fn role_name(role: &ConversationRole) -> &'static str {
    match role {
        ConversationRole::User => "user",
        ConversationRole::Assistant | ConversationRole::Checkpoint => "assistant",
    }
}

pub(super) fn context_turn(item: &HistoryItem) -> Option<ConversationTurn> {
    match item {
        HistoryItem::Event {
            envelope:
                ThreadEventEnvelope {
                    event: ThreadEvent::ContextSuspended { progress },
                    ..
                },
        } => Some(ConversationTurn {
            role: ConversationRole::Assistant,
            text: progress.clone(),
        }),
        // A steered message is the user's words, in the order the model saw them.
        HistoryItem::Event {
            envelope:
                ThreadEventEnvelope {
                    event: ThreadEvent::Steered { text, .. },
                    ..
                },
        } => Some(ConversationTurn {
            role: ConversationRole::User,
            text: text.clone(),
        }),
        HistoryItem::User { text, .. } => Some(ConversationTurn {
            role: ConversationRole::User,
            text: text.clone(),
        }),
        HistoryItem::Event {
            envelope:
                ThreadEventEnvelope {
                    event:
                        ThreadEvent::Finished { result, .. } | ThreadEvent::Incomplete { result, .. },
                    ..
                },
        } => Some(ConversationTurn {
            role: ConversationRole::Assistant,
            text: result.clone(),
        }),
        // Without it the next turn sees the stopped request unanswered and does it again.
        HistoryItem::Event {
            envelope:
                ThreadEventEnvelope {
                    event: ThreadEvent::Failed { error },
                    ..
                },
        } if error.starts_with(themis_core::runtime::STOPPED_BY_YOU) => Some(ConversationTurn {
            role: ConversationRole::Assistant,
            text: themis_core::prompts::RUNTIME_STOPPED_TURN.to_owned(),
        }),
        HistoryItem::Legacy { message } => {
            let role = match message.get("role")?.as_str()? {
                "user" => ConversationRole::User,
                "assistant" if message.get("final")?.as_bool()? => ConversationRole::Assistant,
                _ => return None,
            };
            Some(ConversationTurn {
                role,
                text: message.get("text")?.as_str()?.to_owned(),
            })
        }
        _ => None,
    }
}
