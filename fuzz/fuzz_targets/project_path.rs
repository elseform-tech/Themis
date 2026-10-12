//! Monitor paths come from the UI: no input may resolve outside the project,
//! including through a symlink that points out of it.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::path::PathBuf;
use std::sync::OnceLock;

struct Fixture {
    root: PathBuf,
    _project: tempfile::TempDir,
    _outside: tempfile::TempDir,
}

fn fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src/inner")).unwrap();
        std::fs::write(root.join("src/a.txt"), "a").unwrap();
        std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), root.join("out")).unwrap();
        Fixture { root, _project: project, _outside: outside }
    })
}

fuzz_target!(|relative: &str| {
    let fixture = fixture();
    if let Ok(resolved) = themis_desktop::state::resolve_inside(&fixture.root, relative) {
        assert!(resolved.starts_with(&fixture.root), "{relative:?} -> {resolved:?}");
    }
});
