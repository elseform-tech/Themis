use super::super::package::{Limits, LIMITS, MAX_INLINE_FILE_BYTES};
use super::*;

fn folder(files: &[(&str, &[u8])]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let manifest = root.path().join(".claude-plugin/plugin.json");
    fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    fs::write(manifest, r#"{"name": "big"}"#).unwrap();
    for (path, content) in files {
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    root
}

const SKILL: &[u8] = b"---\nname: s\ndescription: d\n---\nBody";

#[test]
fn default_enabled_false_arrives_switched_off() {
    let root = folder(&[("skills/s/SKILL.md", SKILL)]);
    let manifest = root.path().join(".claude-plugin/plugin.json");
    fs::write(&manifest, r#"{"name": "big", "defaultEnabled": false}"#).unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = PluginStore::new(data.path().into(), None).with_home(None);
    let plugin = store.import_path("global", root.path(), None).unwrap();
    assert!(!plugin.enabled);
    assert!(
        plugin.spec.unsupported.is_empty(),
        "{:?}",
        plugin.spec.unsupported
    );
    fs::write(&manifest, r#"{"name": "other", "defaultEnabled": true}"#).unwrap();
    let plugin = store.import_path("global", root.path(), None).unwrap();
    assert!(plugin.enabled);
}

#[test]
fn an_output_style_is_a_manual_skill_and_says_what_it_is() {
    let style: &[u8] = b"---\nname: Terse\ndescription: Short\n---\nBe brief.";
    let root = folder(&[
        ("skills/s/SKILL.md", SKILL),
        ("output-styles/terse.md", style),
    ]);
    let spec = import_directory(root.path(), true, None, None).unwrap();
    let skill = spec
        .skills
        .iter()
        .find(|skill| skill.id == "style-terse")
        .unwrap();
    assert!(
        skill.instructions.ends_with("Be brief."),
        "{}",
        skill.instructions
    );
    assert!(spec.manual_skills.contains(&"style-terse".to_owned()));
    assert!(spec.unsupported.is_empty(), "{:?}", spec.unsupported);
    assert!(spec
        .notices
        .iter()
        .any(|note| note.contains("for one turn")));
}

#[test]
fn a_servers_cwd_stays_inside() {
    let server = |cwd: &str| McpServer {
        command: Some("x".into()),
        cwd: Some(cwd.into()),
        ..Default::default()
    };
    for good in [
        ".",
        "sub/dir",
        "${CLAUDE_PLUGIN_ROOT}",
        "${CLAUDE_PLUGIN_ROOT}/x",
    ] {
        assert!(server(good).validate().is_ok(), "{good}");
    }
    for bad in [
        "..",
        "../x",
        "/etc",
        "${CLAUDE_PLUGIN_ROOT}/../x",
        "a/../../b",
    ] {
        assert!(server(bad).validate().is_err(), "{bad}");
    }
}

#[test]
fn a_small_text_plugin_stays_in_the_registry_as_before() {
    let root = folder(&[("skills/s/SKILL.md", SKILL), ("skills/s/notes.txt", b"n")]);
    let spec = import_directory(root.path(), true, None, None).unwrap();
    assert!(spec.package.is_none());
    assert!(spec.files.contains_key("skills/s/notes.txt"));
    assert!(spec.unsupported.is_empty(), "{:?}", spec.unsupported);
}

#[test]
fn a_large_or_binary_file_moves_the_folder_out_of_the_registry() {
    let large = vec![b'x'; MAX_INLINE_FILE_BYTES as usize + 1];
    for (name, content) in [
        ("vendor/big.js", large.as_slice()),
        ("bin.dat", &[0xff, 0xfe]),
    ] {
        let root = folder(&[("skills/s/SKILL.md", SKILL), (name, content)]);
        let spec = import_directory(root.path(), true, None, None).unwrap();
        let package = spec.package.expect(name);
        assert_eq!(package.files, 3);
        assert!(!spec.files.contains_key(name));
        // What the importer reads stays, so the skill is found.
        assert_eq!(spec.skills.len(), 1);
        assert!(spec.unsupported.is_empty(), "{:?}", spec.unsupported);
        assert!(spec.notices.is_empty(), "{:?}", spec.notices);
    }
}

#[test]
fn a_check_only_hashes_and_an_install_keeps_the_files() {
    let root = folder(&[("skills/s/SKILL.md", SKILL), ("bin.dat", &[0xff])]);
    let store = tempfile::tempdir().unwrap();
    let checked = import_directory(root.path(), true, None, None).unwrap();
    assert_eq!(fs::read_dir(store.path()).unwrap().count(), 0);
    let kept = import_directory(root.path(), true, None, Some(store.path())).unwrap();
    assert_eq!(checked.package, kept.package);
    let hash = kept.package.unwrap().hash;
    assert_eq!(
        fs::read(store.path().join(hash).join("bin.dat")).unwrap(),
        [0xff]
    );
}

#[test]
fn past_the_bounds_a_plugin_keeps_only_its_own_folders_and_says_so() {
    let big = vec![b'x'; 1024];
    let root = folder(&[
        ("skills/s/SKILL.md", SKILL),
        ("src/a.bin", &big),
        ("src/b.bin", &big),
    ]);
    for limits in [
        Limits {
            total_bytes: 1500,
            ..LIMITS
        },
        Limits { files: 3, ..LIMITS },
    ] {
        let spec = import_directory_within(root.path(), true, None, None, limits).unwrap();
        assert!(!spec.files.contains_key("src/a.bin"));
        assert!(
            spec.unsupported
                .iter()
                .any(|reason| reason.starts_with("Large repository")),
            "{:?}",
            spec.unsupported
        );
        assert_eq!(spec.skills.len(), 1);
    }
    // A single file past the bound is left out, with a note, and the rest is kept.
    let limits = Limits {
        file_bytes: 100,
        ..LIMITS
    };
    let spec = import_directory_within(root.path(), true, None, None, limits).unwrap();
    assert!(spec
        .notices
        .iter()
        .any(|note| note == "File too large: src/a.bin"));
}

#[test]
fn a_plugin_too_large_even_for_its_own_folders_fails() {
    let root = folder(&[
        ("skills/s/SKILL.md", SKILL),
        ("skills/s/x.bin", &[0u8; 2048]),
    ]);
    let limits = Limits {
        total_bytes: 1024,
        ..LIMITS
    };
    let error = import_directory_within(root.path(), true, None, None, limits).unwrap_err();
    assert_eq!(error.to_string(), RESOURCE_LIMITS);
}

/// A check keeps nothing; the install from it downloads the files again, and stops if they
/// aren't what was checked.
#[tokio::test]
async fn installing_a_checked_package_keeps_it_or_stops_if_it_changed() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let manifest = source.path().join(".claude-plugin/marketplace.json");
    fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    fs::write(
        manifest,
        r#"{"name": "local", "plugins": [{"name": "big", "source": "./big"}]}"#,
    )
    .unwrap();
    let plugin = source.path().join("big");
    fs::create_dir_all(plugin.join("skills/s")).unwrap();
    fs::write(plugin.join("skills/s/SKILL.md"), SKILL).unwrap();
    fs::write(plugin.join("tool.bin"), [0xff, 0x01]).unwrap();
    let store = PluginStore::new(data.path().into(), None).with_home(None);
    store
        .add_marketplace("local", source.path().to_str().unwrap())
        .unwrap();
    store.wait_marketplace_scan("local", false).await.unwrap();
    let packages = data
        .path()
        .join("plugins")
        .join(super::super::package::PACKAGES_DIR);
    assert!(!packages.exists(), "a check stored the package");
    let installed = store
        .install_scanned("global", "local", "big", false, None)
        .await
        .unwrap();
    let hash = installed.spec.package.as_ref().unwrap().hash.clone();
    assert_eq!(
        fs::read(packages.join(&hash).join("tool.bin")).unwrap(),
        [0xff, 0x01]
    );
    // Materialized for a run, then the store can be read again from the registry alone.
    let project = tempfile::tempdir().unwrap();
    store
        .materialize(std::slice::from_ref(&installed), project.path())
        .unwrap();
    let copied = project
        .path()
        .join(".themis/plugin-files")
        .join(installed.skill_id("resources"))
        .join("tool.bin");
    assert_eq!(fs::read(copied).unwrap(), [0xff, 0x01]);
    // Upstream changed after the check: what was reviewed is no longer what would arrive.
    store.delete("global", "big").unwrap();
    fs::remove_dir_all(packages.join(&hash)).unwrap();
    fs::write(plugin.join("tool.bin"), [0xff, 0x02]).unwrap();
    let error = store
        .install_scanned("global", "local", "big", false, None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("changed after it was checked"),
        "{error}"
    );
    assert_eq!(fs::read_dir(&packages).unwrap().count(), 0);
}

#[tokio::test]
async fn a_download_past_its_limit_is_refused() {
    let repository = folder(&[("big.bin", &[1u8; 4096])]);
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "x",
        ],
    ] {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(repository.path())
            .status()
            .unwrap()
            .success());
    }
    let target = tempfile::tempdir().unwrap();
    let path = target.path().join("copy");
    let source = repository.path().to_str().unwrap();
    let args = ["clone", "--depth=1", "--", source, path.to_str().unwrap()];
    let error = run_git_within(&args, None, Some((path.as_path(), 1024)))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), REPOSITORY_TOO_LARGE);
}
