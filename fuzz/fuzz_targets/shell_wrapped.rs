//! Model-written shell arguments are untrusted: reading commands inside wrappers
//! (`sh -c`, `sudo`, `eval`, ...) must never panic or grow without bound, and a
//! readable wrapper always yields non-empty commands.
#![no_main]
use libfuzzer_sys::fuzz_target;
use themis_core::tools::fuzzing::wrapped_commands;

fuzz_target!(|data: (u8, Vec<String>)| {
    let (shape, mut words) = data;
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
