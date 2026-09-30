use std::process::Command;

#[test]
fn creator_verifier_accepts_valid_skill_and_rejects_unsafe_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skill.json");
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/builtins/create-skill/scripts/verify.py"
    );
    std::fs::write(&path,r#"{"id":"review","name":"Review","description":"Inspect changes","instructions":"Inspect changes","allowedTools":[],"scripts":[]}"#).unwrap();
    assert!(Command::new("python3")
        .arg(script)
        .arg(&path)
        .status()
        .unwrap()
        .success());
    std::fs::write(&path,r#"{"id":"review","name":"Review","description":"Inspect changes","instructions":"Inspect changes","allowedTools":[],"scripts":[{"name":"../escape.py","content":"pass"}]}"#).unwrap();
    assert!(!Command::new("python3")
        .arg(script)
        .arg(&path)
        .status()
        .unwrap()
        .success());
}

#[test]
fn creator_bundle_is_resolved_and_materializes_its_verifier() {
    let dir = tempfile::tempdir().unwrap();
    let store = themis_core::plugins::PluginStore::new(
        dir.path().join("global"),
        Some(dir.path().to_owned()),
    );
    let (_, skills, _) = store
        .resolve_prompt("[[skill:create-skill]] Make a local review skill", &[])
        .unwrap();
    assert_eq!(skills[0].id, "create-skill");
    assert!(skills[0].instructions.contains("save_skill"));
    assert!(skills[0].instructions.contains("verify.py"));
    themis_core::skills::materialize_scripts(&skills, dir.path()).unwrap();
    let verifier = dir.path().join(".themis/skills/create-skill/verify.py");
    assert!(verifier.is_file());
    let output = Command::new("python3")
        .arg(verifier)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/builtins/create-skill"
        ))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
