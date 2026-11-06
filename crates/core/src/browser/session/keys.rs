//! Key descriptions such as `Control+a`, parsed for the browser.

/// A key press, parsed from `Enter`, `Control+a`, `Meta+Shift+k`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPress {
    pub key: String,
    pub code: String,
    pub key_code: u32,
    pub text: Option<String>,
    pub modifiers: u32,
}

/// Parses a key description; `None` for unknown keys.
pub fn parse_key(description: &str) -> Option<KeyPress> {
    let parts: Vec<&str> = description.split('+').map(str::trim).collect();
    let (key, modifiers) = match parts.split_last() {
        // "Control++" means the plus key ("Control+" names no key).
        Some((&"", rest)) => match rest.split_last() {
            Some((&"", modifiers)) => ("+", modifiers),
            _ => return None,
        },
        Some((last, rest)) => (*last, rest),
        None => return None,
    };
    // Web pages name keys KeyW and Digit1, so models do too.
    let from_code = key
        .strip_prefix("Key")
        .filter(|rest| rest.len() == 1 && rest.chars().all(|c| c.is_ascii_alphabetic()))
        .or_else(|| {
            key.strip_prefix("Digit")
                .filter(|rest| rest.len() == 1 && rest.chars().all(|c| c.is_ascii_digit()))
        })
        .map(str::to_ascii_lowercase);
    let key = from_code.as_deref().unwrap_or(key);
    let mut mask = 0;
    for modifier in modifiers {
        mask |= match modifier.to_ascii_lowercase().as_str() {
            "alt" | "option" => 1,
            "control" | "ctrl" => 2,
            "meta" | "cmd" | "command" => 4,
            "shift" => 8,
            _ => return None,
        };
    }
    let named = |key: &str, code: &str, key_code: u32, text: Option<&str>| KeyPress {
        key: key.to_owned(),
        code: code.to_owned(),
        key_code,
        text: text.map(str::to_owned),
        modifiers: mask,
    };
    let press = match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => named("Enter", "Enter", 13, Some("\r")),
        "tab" => named("Tab", "Tab", 9, None),
        "escape" | "esc" => named("Escape", "Escape", 27, None),
        "backspace" => named("Backspace", "Backspace", 8, None),
        "delete" => named("Delete", "Delete", 46, None),
        "space" | " " => named(" ", "Space", 32, Some(" ")),
        "arrowup" | "up" => named("ArrowUp", "ArrowUp", 38, None),
        "arrowdown" | "down" => named("ArrowDown", "ArrowDown", 40, None),
        "arrowleft" | "left" => named("ArrowLeft", "ArrowLeft", 37, None),
        "arrowright" | "right" => named("ArrowRight", "ArrowRight", 39, None),
        "home" => named("Home", "Home", 36, None),
        "end" => named("End", "End", 35, None),
        "pageup" => named("PageUp", "PageUp", 33, None),
        "pagedown" => named("PageDown", "PageDown", 34, None),
        lower
            if lower.len() > 1
                && lower.starts_with('f')
                && lower[1..]
                    .parse::<u32>()
                    .is_ok_and(|n| (1..=12).contains(&n)) =>
        {
            let number: u32 = lower[1..].parse().unwrap_or(1);
            named(
                &format!("F{number}"),
                &format!("F{number}"),
                111 + number,
                None,
            )
        }
        _ => {
            let mut chars = key.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            let shifted = mask & 8 != 0;
            let shown = if shifted { c.to_ascii_uppercase() } else { c };
            let code = if c.is_ascii_alphabetic() {
                format!("Key{}", c.to_ascii_uppercase())
            } else if c.is_ascii_digit() {
                format!("Digit{c}")
            } else {
                String::new()
            };
            let key_code = if c.is_ascii_alphanumeric() {
                u32::from(c.to_ascii_uppercase())
            } else {
                0
            };
            // Shortcuts (Control/Meta/Alt) type nothing.
            let text = (mask & 7 == 0).then(|| shown.to_string());
            KeyPress {
                key: shown.to_string(),
                code,
                key_code,
                text,
                modifiers: mask,
            }
        }
    };
    Some(press)
}
