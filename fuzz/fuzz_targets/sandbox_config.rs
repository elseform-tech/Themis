//! Sandbox configuration, Seatbelt profiles and denial output are untrusted
//! text: a user and a project layer must never panic the resolver and a
//! project layer must never loosen the sandbox; no path ever reaches the
//! Seatbelt profile text; and output scanning stays bounded.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::path::PathBuf;
use themis_core::configuration::{check_file, resolve};
use themis_core::sandbox::fuzzing::{path_candidates, seatbelt_profile};
use themis_core::sandbox::NetworkPolicy;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let mut parts = text.splitn(3, '\u{0}');
    let user = parts.next().unwrap_or_default();
    let project = parts.next().unwrap_or_default();
    let output = parts.next().unwrap_or_default();

    let _ = check_file(user);
    let _ = check_file(project);
    if let (Ok(alone), Ok(both)) = (resolve(Some(user), None, None), resolve(Some(user), Some(project), None)) {
        let (alone, both) = (&alone.config.sandbox, &both.config.sandbox);
        assert!(!alone.enabled || both.enabled, "a project disabled the sandbox");
        assert_eq!(alone.backend, both.backend, "a project changed the backend");
        assert!(alone.network == NetworkPolicy::Allow || both.network == NetworkPolicy::Block);
        for path in &both.writable_paths {
            assert!(alone.writable_paths.contains(path), "a project added a writable path");
        }
    }

    let paths: Vec<PathBuf> = output
        .lines()
        .take(8)
        .filter(|line| line.starts_with('/') && line.len() > 3)
        .map(PathBuf::from)
        .collect();
    let block = output.len() % 2 == 0;
    let (profile, params) = seatbelt_profile(&paths, &paths, &paths, block);
    let (fixed, _) = seatbelt_profile(&[], &[], &[], block);
    assert_eq!(params.len(), paths.len() * 3);
    for path in &paths {
        let path = path.to_string_lossy();
        // Device rules such as `/dev/null` are part of every profile.
        if !fixed.contains(path.as_ref()) {
            assert!(!profile.contains(path.as_ref()), "a path leaked into the profile text");
        }
    }
    for line in output.lines().take(64) {
        let found = path_candidates(line);
        assert!(found.len() <= 16);
        assert!(found.iter().all(|candidate| line.contains(candidate.as_str())));
    }
});
