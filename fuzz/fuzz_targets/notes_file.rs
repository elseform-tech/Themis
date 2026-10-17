//! The notes file sits in the project and the update comes from the model: both are
//! untrusted. Parsing and applying must never panic, and the rendered block stays bounded
//! with exactly one frame end, so a note cannot forge the end of its own block.
#![no_main]
use libfuzzer_sys::fuzz_target;
use themis_core::runtime::fuzzing::notes_file_and_update;

fuzz_target!(|data: (Vec<u8>, String)| {
    let (file, update) = data;
    let (rendered, _) = notes_file_and_update(&file, &update);
    assert!(rendered.len() <= 8192);
    assert!(rendered.is_empty() || rendered.matches("\nEnd saved notes.\n").count() == 1);
});
