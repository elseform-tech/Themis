//! Chat titles made from the first message.

use super::*;

pub(super) fn title_from_message(text: &str) -> String {
    // A chat started with `/goal` is named after the goal, not the command.
    let goal = themis_core::goal::parse_goal_command(text);
    let text = goal.as_deref().unwrap_or(text);
    // So is one whose first message is the runtime's goal turn, set up from outside the composer.
    let text = text
        .strip_prefix(themis_core::goal::GOAL_TURN_MARKER)
        .and_then(|rest| rest.lines().next())
        .and_then(|line| line.split_once(": "))
        .map_or(text, |(_, goal)| goal);
    let first = text.lines().next().unwrap_or_default();
    // Composer chips (`[[skill:id]]`, `[[plugin:id]]`) are not words.
    let (words, ids) = without_tokens(first);
    let title = if words.is_empty() {
        ids.join(" ")
    } else {
        words
    };
    if title.is_empty() {
        return UNTITLED_THREAD.to_owned();
    }
    let title: String = title.chars().take(MAX_TITLE_CHARS).collect();
    title.trim_end().to_owned()
}

/// `text` without skill/plugin tokens (whitespace collapsed), and their ids.
pub(super) fn without_tokens(text: &str) -> (String, Vec<&str>) {
    let mut kept = String::new();
    let mut ids = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let id = after.split_once("]]").and_then(|(inner, _)| {
            let (kind, id) = inner.split_once(':')?;
            let valid = matches!(kind, "skill" | "plugin")
                && !id.is_empty()
                && !id.contains(char::is_whitespace);
            valid.then_some((inner.len(), id))
        });
        match id {
            Some((len, id)) => {
                kept.push_str(&rest[..start]);
                kept.push(' ');
                ids.push(id);
                rest = &after[len + 2..];
            }
            None => {
                kept.push_str(&rest[..start + 2]);
                rest = after;
            }
        }
    }
    kept.push_str(rest);
    (kept.split_whitespace().collect::<Vec<_>>().join(" "), ids)
}
