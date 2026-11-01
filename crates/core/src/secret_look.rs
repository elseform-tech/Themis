//! Whether a person's message looks like it holds a credential. Same rules as
//! `web/src/lib/secretLook.ts`, which warns in the composer; `web/src/lib/secretSamples.json`
//! holds the cases both must agree on. Nothing here logs or returns what it found.

use regex::Regex;
use std::sync::LazyLock;

/// Characters scanned: a key pasted past this point of a very long message is missed.
const MAX_SCAN_BYTES: usize = 256 * 1024;
/// Shorter values next to a name such as `API_KEY=` are not worth a warning.
const MIN_VALUE_CHARS: usize = 20;
/// Bits per character below which a value reads as words or a repeated placeholder.
const MIN_ENTROPY: f64 = 3.2;

fn compiled(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid built-in pattern")
}

static FIXED_SHAPES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?:^|[^A-Za-z0-9_-])(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{30,}",
        r"(?:^|[^A-Za-z0-9_-])github_pat_[A-Za-z0-9_]{30,}",
        r"(?:^|[^A-Za-z0-9_-])xox[baprs]-[0-9]+-[A-Za-z0-9-]{10,}",
        r"(?:^|[^A-Za-z0-9_-])AKIA[0-9A-Z]{16}(?:$|[^A-Za-z0-9])",
        r"(?:^|[^A-Za-z0-9_-])AIza[0-9A-Za-z_-]{35}(?:$|[^A-Za-z0-9_-])",
        r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----",
        r"(?:^|[^A-Za-z0-9_-])eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
    ]
    .iter()
    .map(|pattern| compiled(pattern))
    .collect()
});
static OPENAI_STYLE: LazyLock<Regex> =
    LazyLock::new(|| compiled(r"(?:^|[^A-Za-z0-9_-])sk-([A-Za-z0-9_-]{20,})"));
static NAMED_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    compiled(
        r#"(?:^|[^A-Za-z0-9_])[A-Z0-9_]*(?:KEY|TOKEN|SECRET|PASSWORD|PASSWD|CREDENTIAL)[A-Z0-9_]*[ \t]*[=:][ \t]*["']?([A-Za-z0-9+/_.=-]{20,})"#,
    )
});
static UUID_SHAPE: LazyLock<Regex> = LazyLock::new(|| {
    compiled(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
});

/// Bits per character of `text`.
fn entropy(text: &str) -> f64 {
    let mut counts = std::collections::HashMap::new();
    let mut total = 0_u32;
    for c in text.chars() {
        *counts.entry(c).or_insert(0_u32) += 1;
        total += 1;
    }
    counts
        .values()
        .map(|count| {
            let share = f64::from(*count) / f64::from(total);
            -share * share.log2()
        })
        .sum()
}

/// Both letters and digits, varied enough not to be words or a placeholder.
fn looks_random(value: &str) -> bool {
    value.chars().count() >= MIN_VALUE_CHARS
        && value.chars().any(|c| c.is_ascii_digit())
        && value.chars().any(|c| c.is_ascii_alphabetic())
        && entropy(value) >= MIN_ENTROPY
        && !UUID_SHAPE.is_match(value)
}

/// Whether `text` looks like it holds an API key, token or private key.
#[must_use]
pub fn looks_like_secret(text: &str) -> bool {
    let mut end = text.len().min(MAX_SCAN_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let text = &text[..end];
    if FIXED_SHAPES.iter().any(|shape| shape.is_match(text)) {
        return true;
    }
    // `sk-` also starts CSS class names and the like: one long random part is what a key has.
    let openai = OPENAI_STYLE.captures_iter(text).any(|found| {
        let tail = &found[1];
        looks_random(tail) && tail.split('-').any(|part| part.len() >= 16)
    });
    openai
        || NAMED_VALUE
            .captures_iter(text)
            .any(|found| looks_random(&found[1]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::Value;

    const SAMPLES: &str = include_str!("../../../web/src/lib/secretSamples.json");

    fn samples() -> Value {
        serde_json::from_str(SAMPLES).unwrap()
    }

    fn joined(pieces: &Value) -> String {
        pieces
            .as_array()
            .unwrap()
            .iter()
            .map(|piece| piece.as_str().unwrap())
            .collect()
    }

    #[test]
    fn real_shapes_are_found() {
        for pieces in samples()["secrets"].as_array().unwrap() {
            let text = joined(pieces);
            assert!(looks_like_secret(&text), "{text}");
        }
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        for text in samples()["ordinary"].as_array().unwrap() {
            let text = text.as_str().unwrap();
            assert!(!looks_like_secret(text), "{text}");
        }
    }

    #[test]
    fn a_key_past_the_scanned_part_is_not_looked_for() {
        let key = joined(&samples()["secrets"][4]);
        assert!(looks_like_secret(&format!("{}{key}", " ".repeat(1000))));
        assert!(!looks_like_secret(&format!(
            "{}{key}",
            " ".repeat(MAX_SCAN_BYTES)
        )));
        // A cut inside a multi-byte character does not panic.
        let _ = looks_like_secret(&"é".repeat(MAX_SCAN_BYTES));
    }

    proptest! {
        #[test]
        fn any_text_is_read_without_a_panic(text in ".*") {
            let _ = looks_like_secret(&text);
        }

        #[test]
        fn hashes_uuids_and_words_never_look_like_secrets(
            hash in "[0-9a-f]{40,64}",
            uuid in "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
            word in "[a-z]{20,60}",
        ) {
            let (a, b, c, d) = (format!("the commit {hash} is fine"), format!("id {uuid}"), format!("TOKEN_ID={uuid}"), format!("sk-{word}"));
            prop_assert!(!looks_like_secret(&a));
            prop_assert!(!looks_like_secret(&b));
            prop_assert!(!looks_like_secret(&c));
            prop_assert!(!looks_like_secret(&word));
            prop_assert!(!looks_like_secret(&d));
        }
    }
}
