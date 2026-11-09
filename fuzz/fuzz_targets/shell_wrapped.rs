//! Model-written shell arguments are untrusted: reading commands inside wrappers
//! (`sh -c`, `sudo`, `eval`, ...) must never panic or grow without bound, and a
//! readable wrapper always yields non-empty commands. A script the read-only check
//! reads as one plain command holds no unquoted shell character, and its words,
//! single-quoted and joined again, read back the same.
#![no_main]
use libfuzzer_sys::fuzz_target;
use themis_core::tools::fuzzing::{plain_words, wrapped_commands};

fuzz_target!(|data: (u8, Vec<String>)| {
    let (shape, mut words) = data;
    if let Some(script) = words.first() {
        check_plain(script);
    }
    // Steer about half of the inputs into a wrapper so the script reader is exercised.
    let wrappers: [&[&str]; 6] = [
        &["sh", "-c"],
        &["bash", "-lc"],
        &["sudo", "-u", "x"],
        &["env", "A=1"],
        &["eval"],
        &["cmd", "/c"],
    ];
    if let Some(prefix) = wrappers.get(shape as usize % 12) {
        let mut wrapped: Vec<String> = prefix.iter().map(|w| w.to_string()).collect();
        wrapped.append(&mut words);
        words = wrapped;
    }
    if let Some(found) = wrapped_commands(&words) {
        assert!(found.len() <= 64);
        assert!(found.iter().all(|command| !command.is_empty()));
    }
});

fn check_plain(script: &str) {
    let Some(words) = plain_words(script) else {
        return;
    };
    assert!(!words.is_empty());
    assert!(!words[0].contains('='));
    // Outside quotes only plain characters may appear.
    let mut quote: Option<char> = None;
    for c in script.chars() {
        match quote {
            Some(open) if c == open => quote = None,
            Some('"') => assert!(!matches!(c, '$' | '`' | '\\' | '!'), "{script:?}"),
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None => assert!(
                c == ' ' || c == '\t' || c.is_ascii_alphanumeric() || "-_./:,@+=%".contains(c),
                "{script:?}"
            ),
        }
    }
    assert!(quote.is_none(), "{script:?}");
    if words.iter().all(|word| !word.contains('\'')) {
        let quoted: Vec<String> = words.iter().map(|word| format!("'{word}'")).collect();
        assert_eq!(plain_words(&quoted.join(" ")).as_ref(), Some(&words));
    }
}
