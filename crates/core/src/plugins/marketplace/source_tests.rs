use super::*;
use proptest::prelude::*;

#[test]
fn only_bounded_svg_png_and_webp_become_icons() {
    let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[0; 16]].concat();
    assert!(icon_asset_name(&png).unwrap().ends_with(".png"));
    let webp = [b"RIFF\0\0\0\0WEBP".as_slice(), &[0; 16]].concat();
    assert!(icon_asset_name(&webp).unwrap().ends_with(".webp"));
    assert!(icon_asset_name(b"  <svg xmlns='x'/>")
        .unwrap()
        .ends_with(".svg"));
    for bad in [b"".as_slice(), b"<html></html>", b"GIF89a", b"#!/bin/sh"] {
        assert!(icon_asset_name(bad).is_none());
    }
    let huge = [b"<svg>".as_slice(), &vec![b' '; ICON_LIMIT]].concat();
    assert!(icon_asset_name(&huge).is_none());
}

#[test]
fn typed_sources_normalize_to_stored_ones() {
    assert_eq!(
        normalize_source(" acme/agent-plugins ").unwrap(),
        "https://github.com/acme/agent-plugins.git"
    );
    assert_eq!(
        normalize_source("https://example.com/a/b.git").unwrap(),
        "https://example.com/a/b.git"
    );
    for bad in [
        "",
        "plugins",
        "../up/here",
        "a/b/c",
        "http://example.com/a.git",
    ] {
        assert!(normalize_source(bad).is_err(), "{bad}");
    }
}

proptest! {
    /// Whatever is typed, what gets stored is an HTTPS URL without credentials, or an
    /// absolute path. Nothing relative, and nothing that git could take as an option.
    #[test]
    fn stored_sources_are_https_or_absolute(input in "\\PC{0,40}") {
        if let Ok(source) = normalize_source(&input) {
            prop_assert!(source.starts_with("https://") || Path::new(&source).is_absolute());
            prop_assert!(!source.starts_with('-'));
            if source.starts_with("https://") {
                let url = reqwest::Url::parse(&source).unwrap();
                prop_assert!(url.username().is_empty() && url.password().is_none());
            }
        }
    }
}

/// Only the manifest is fetched, however large the repository.
#[cfg(unix)]
#[tokio::test]
async fn a_git_source_fetches_only_its_manifest() {
    let repository = tempfile::tempdir().unwrap();
    let root = repository.path();
    fs::create_dir_all(root.join(".claude-plugin")).unwrap();
    fs::write(
        root.join(".claude-plugin/marketplace.json"),
        r#"{"name":"acme","plugins":[{"name":"one"}]}"#,
    )
    .unwrap();
    fs::write(root.join("huge.bin"), "bulk").unwrap();
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
            .args(&args)
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }
    let out = tempfile::tempdir().unwrap();
    let staging = out.path().join("preview");
    let url = format!("file://{}", root.display());
    let directory = read_git_manifest(&url, &staging).await.unwrap();
    assert!(!directory.join("huge.bin").exists());
    let preview = source_preview(&url, &directory).unwrap();
    assert_eq!((preview.name.as_str(), preview.plugins), ("acme", 1));
    let missing = read_git_manifest(&format!("{url}-missing"), &out.path().join("other"))
        .await
        .unwrap_err();
    assert_eq!(missing.to_string(), "This source can't be reached.");
}
