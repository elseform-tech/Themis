//! A model-written patch is untrusted text: parsing and the forgiving applier must never
//! panic, and a successful apply only produces lines from the file or the patch's additions.
#![no_main]
use libfuzzer_sys::fuzz_target;
use themis_core::tools::fuzzing::apply_hunk_text;

fuzz_target!(|data: (&str, &str)| {
    let (old, patch) = data;
    if let Ok(result) = apply_hunk_text(old, patch) {
        let added: Vec<&str> = patch
            .lines()
            .filter_map(|line| line.strip_prefix('+'))
            .collect();
        for line in &result {
            assert!(
                old.lines().any(|original| original == line) || added.contains(&line.as_str()),
                "line {line:?} came from neither the file nor the patch"
            );
        }
    }
});
