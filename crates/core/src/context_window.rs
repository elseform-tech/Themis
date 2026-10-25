//! The model's context window: what the run may fill, and where that number
//! came from.
//!
//! The context budget is Auto by default: it follows the selected model's
//! window, up to [`AUTO_CONTEXT_CEILING`]. A configured number is a cap that
//! is still clamped to the window. The window comes from the public model
//! catalog; when the catalog does not list it, a moderate guess
//! ([`UNKNOWN_WINDOW_GUESS`]) stands in until a provider's context-length
//! error teaches the real limit, which is remembered per model for the rest
//! of the process. Holds numbers and model ids only.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Most tokens an Auto budget uses, however large the model's window. The one
/// place this ceiling is set.
pub const AUTO_CONTEXT_CEILING: usize = 200_000;
/// Window assumed for a model the catalog does not describe, until a
/// context-length error teaches the real one.
pub const UNKNOWN_WINDOW_GUESS: usize = 128_000;
/// A goal starting with a smaller effective window gets a one-time warning.
pub const SMALL_WINDOW_TOKENS: usize = 128_000;
/// Smallest window a context-length error can teach: below this a run cannot
/// hold its tools and an answer anyway.
pub const MIN_LEARNED_WINDOW: usize = 8_000;
/// Largest configurable cap and largest window this module believes.
pub const MAX_CONTEXT_TOKENS: usize = 1_000_000;
/// Share of a failed request's size kept when the error did not state the
/// limit (in percent).
const UNSTATED_LIMIT_PERCENT: usize = 90;
/// Most error text inspected for a stated limit.
const INSPECTED_BYTES: usize = 16 * 1024;
/// Models whose learned limit is remembered (oldest entries are not tracked;
/// the map is cleared when full).
const LEARNED_MODELS: usize = 256;

/// Where the effective window came from (diagnostics `capacity_source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowSource {
    /// The public model catalog listed the window.
    Catalog,
    /// A context-length error taught a smaller window.
    Learned,
    /// Unknown window: the moderate starting guess.
    Guess,
    /// The caller did not report a window (tests, evals, override endpoints).
    Unverified,
}

impl WindowSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Catalog => "catalog",
            Self::Learned => "learned",
            Self::Guess => "guess",
            Self::Unverified => "caller_budget_unverified",
        }
    }
}

/// The model's window as far as this process knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// `None` only when the caller reported no window (unverified).
    pub tokens: Option<usize>,
    pub source: WindowSource,
}

fn learned() -> &'static Mutex<HashMap<String, usize>> {
    static LEARNED: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    LEARNED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The limit a context-length error taught for `model`, if any.
#[must_use]
pub fn learned_limit(model: &str) -> Option<usize> {
    learned()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(model)
        .copied()
}

/// Remembers that `model` rejected requests above `tokens` (bounded to
/// [`MIN_LEARNED_WINDOW`]..=[`MAX_CONTEXT_TOKENS`]). Only ever lowers a
/// learned limit. Returns the limit now remembered.
pub fn learn_limit(model: &str, tokens: usize) -> usize {
    let tokens = tokens.clamp(MIN_LEARNED_WINDOW, MAX_CONTEXT_TOKENS);
    let mut map = learned().lock().unwrap_or_else(|error| error.into_inner());
    if map.len() >= LEARNED_MODELS && !map.contains_key(model) {
        map.clear();
    }
    let entry = map.entry(model.to_owned()).or_insert(tokens);
    *entry = (*entry).min(tokens);
    *entry
}

/// Forgets every learned limit (tests).
pub fn forget_learned_limits() {
    learned()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clear();
}

/// The window for `model`. `reported` is what the caller knows: `None` when it
/// reports nothing (unverified), `Some(None)` when the catalog does not list
/// the model, `Some(Some(n))` for a catalog window. A learned limit lowers a
/// guess or a catalog window.
#[must_use]
pub fn window(model: &str, reported: Option<Option<usize>>) -> Window {
    let Some(catalog) = reported else {
        return Window {
            tokens: None,
            source: WindowSource::Unverified,
        };
    };
    let (base, source) = match catalog {
        Some(tokens) => (tokens.min(MAX_CONTEXT_TOKENS), WindowSource::Catalog),
        None => (UNKNOWN_WINDOW_GUESS, WindowSource::Guess),
    };
    match learned_limit(model) {
        Some(limit) if limit < base => Window {
            tokens: Some(limit),
            source: WindowSource::Learned,
        },
        _ => Window {
            tokens: Some(base),
            source,
        },
    }
}

/// The budget a run uses: the configured cap (Auto is
/// [`AUTO_CONTEXT_CEILING`]) clamped to the window. `usize::MAX` means
/// unbounded and is returned unchanged.
#[must_use]
pub fn effective_budget(cap: usize, window: Window) -> usize {
    if cap == usize::MAX {
        return cap;
    }
    window.tokens.map_or(cap, |tokens| cap.min(tokens))
}

/// The window a context-length error implies for a request of
/// `request_tokens` (messages, tool definitions and output reserve): the limit
/// the error states, or a little less than the failed request.
#[must_use]
pub fn implied_limit(error_text: &str, request_tokens: usize) -> usize {
    stated_limit(error_text)
        .unwrap_or_else(|| request_tokens.saturating_mul(UNSTATED_LIMIT_PERCENT) / 100)
}

/// The context limit a provider error states, such as "maximum context length
/// is 8192 tokens" or "context window of 32,768 tokens". Reads a bounded
/// prefix; returns only plausible window sizes
/// ([`MIN_LEARNED_WINDOW`]..=[`MAX_CONTEXT_TOKENS`]).
#[must_use]
pub fn stated_limit(text: &str) -> Option<usize> {
    let mut end = text.len().min(INSPECTED_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let lower = text[..end].to_ascii_lowercase();
    const ANCHORS: &[&str] = &[
        "maximum context length",
        "context length",
        "context window",
        "maximum context",
        "context limit",
        "max context",
        "maximum prompt length",
    ];
    for anchor in ANCHORS {
        let mut from = 0;
        while let Some(found) = lower[from..].find(anchor) {
            let start = from + found + anchor.len();
            // The number must follow directly ("is 8192", "of 32,768"), not
            // come from somewhere later in the message.
            if let Some(number) = leading_number(connected(&lower[start..])) {
                if (MIN_LEARNED_WINDOW..=MAX_CONTEXT_TOKENS).contains(&number) {
                    return Some(number);
                }
            }
            from = start;
        }
    }
    None
}

/// `text` after the connecting words between an anchor and its number
/// ("is", "of", ":", "=", "limit", "(").
fn connected(mut text: &str) -> &str {
    loop {
        let trimmed = text.trim_start();
        let rest = ["is", "of", "limit", ":", "=", "("]
            .iter()
            .find_map(|word| trimmed.strip_prefix(word));
        match rest {
            Some(rest) => text = rest,
            None => return trimmed,
        }
    }
}

/// The number at the start of `text`, allowing `,` or `_` group separators.
fn leading_number(text: &str) -> Option<usize> {
    if !text.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let mut value: usize = 0;
    let mut digits = 0;
    for c in text.chars() {
        if let Some(digit) = c.to_digit(10) {
            value = value.checked_mul(10)?.checked_add(digit as usize)?;
            digits += 1;
            if digits > 9 {
                return None;
            }
        } else if c != ',' && c != '_' {
            break;
        }
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn auto_follows_the_window_up_to_the_ceiling() {
        let catalog = |tokens| Window {
            tokens: Some(tokens),
            source: WindowSource::Catalog,
        };
        assert_eq!(
            effective_budget(AUTO_CONTEXT_CEILING, catalog(64_000)),
            64_000
        );
        assert_eq!(
            effective_budget(AUTO_CONTEXT_CEILING, catalog(1_000_000)),
            AUTO_CONTEXT_CEILING
        );
        // A cap is still clamped to the window, and may sit above the Auto ceiling.
        assert_eq!(effective_budget(500_000, catalog(1_000_000)), 500_000);
        assert_eq!(effective_budget(500_000, catalog(128_000)), 128_000);
        assert_eq!(effective_budget(50_000, catalog(128_000)), 50_000);
        assert_eq!(effective_budget(usize::MAX, catalog(8_000)), usize::MAX);
        let unverified = window("m", None);
        assert_eq!(unverified.source, WindowSource::Unverified);
        assert_eq!(effective_budget(300_000, unverified), 300_000);
    }

    #[test]
    fn unknown_window_starts_at_the_guess_and_learned_limits_lower_it() {
        let model = "context-window-test-model";
        forget_learned_limits();
        let unknown = window(model, Some(None));
        assert_eq!(unknown.tokens, Some(UNKNOWN_WINDOW_GUESS));
        assert_eq!(unknown.source.as_str(), "guess");
        assert_eq!(
            window(model, Some(Some(262_144))).source.as_str(),
            "catalog"
        );
        assert_eq!(learn_limit(model, 32_768), 32_768);
        // A later, larger observation never raises what was learned.
        assert_eq!(learn_limit(model, 64_000), 32_768);
        let learned = window(model, Some(None));
        assert_eq!(learned.tokens, Some(32_768));
        assert_eq!(learned.source.as_str(), "learned");
        assert_eq!(window(model, Some(Some(262_144))).tokens, Some(32_768));
        // A learned limit above a catalog window does not raise it.
        assert_eq!(window(model, Some(Some(16_000))).source.as_str(), "catalog");
        assert_eq!(learn_limit(model, 10), MIN_LEARNED_WINDOW);
        forget_learned_limits();
    }

    #[test]
    fn stated_limits_are_read_from_common_provider_errors() {
        for (text, limit) in [
            ("This model's maximum context length is 8192 tokens. However, you requested 9000 tokens", Some(8192)),
            (r#"{"error":{"message":"maximum context length is 131,072 tokens"}}"#, Some(131_072)),
            ("Input exceeds the context window of 32768 tokens", Some(32_768)),
            ("prompt is too long: 210000 tokens > 200000 maximum", None),
            ("context length exceeded", None),
            ("maximum context length is 12 tokens", None),
            ("context window: 9999999999999999999999", None),
            ("context length exceeded; you requested 50000 tokens", None),
            ("context limit (65,536)", Some(65_536)),
        ] {
            assert_eq!(stated_limit(text), limit, "{text}");
        }
        assert_eq!(implied_limit("context length exceeded", 100_000), 90_000);
        assert_eq!(
            implied_limit("maximum context length is 65536", 100_000),
            65_536
        );
    }

    proptest! {
        #[test]
        fn stated_limit_is_total_and_bounded(text in ".{0,400}") {
            if let Some(limit) = stated_limit(&text) {
                prop_assert!((MIN_LEARNED_WINDOW..=MAX_CONTEXT_TOKENS).contains(&limit));
            }
        }

        #[test]
        fn stated_limit_finds_an_embedded_limit(
            prefix in "[a-z .]{0,40}", limit in MIN_LEARNED_WINDOW..=MAX_CONTEXT_TOKENS, suffix in ".{0,40}"
        ) {
            let text = format!("{prefix} maximum context length is {limit} tokens{suffix}");
            prop_assert_eq!(stated_limit(&text), Some(limit));
        }
    }
}
