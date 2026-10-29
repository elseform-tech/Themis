use super::*;
use proptest::prelude::*;
use std::path::PathBuf;

fn config(json: &str) -> SandboxConfig {
    serde_json::from_str(json).unwrap()
}

fn plan_with(
    enforcement: Enforcement,
    config: &SandboxConfig,
    root: &Path,
    app_data: &Path,
    home: &Path,
) -> SandboxPlan {
    SandboxPlan::with_enforcement(
        PlanInputs {
            config,
            project_root: root,
            app_data: Some(app_data),
            home: Some(home),
            open: None,
        },
        enforcement,
        "test".into(),
    )
}

/// A project, app-data and home folder outside the temporary folder, so that
/// "outside the writable folders" really is outside them.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    app_data: PathBuf,
    home: PathBuf,
    outside: PathBuf,
}

fn fixture() -> Fixture {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/sandbox-tests");
    std::fs::create_dir_all(&base).unwrap();
    let dir = tempfile::tempdir_in(&base).unwrap();
    let root = dir.path().join("project");
    let app_data = dir.path().join("app-data");
    let home = dir.path().join("home");
    let outside = dir.path().join("outside");
    for path in [&root, &app_data, &home, &outside, &home.join(".ssh")] {
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::write(home.join(".ssh/id_test"), "secret").unwrap();
    std::fs::write(app_data.join("runtime.jsonc"), "{}").unwrap();
    std::fs::create_dir_all(app_data.join("chat_grants")).unwrap();
    Fixture {
        root: root.canonicalize().unwrap(),
        app_data: app_data.canonicalize().unwrap(),
        home: home.canonicalize().unwrap(),
        outside: outside.canonicalize().unwrap(),
        _dir: dir,
    }
}

#[test]
fn defaults_are_on_auto_with_network() {
    let defaults = SandboxConfig::default();
    assert!(defaults.enabled);
    assert_eq!(defaults.backend, BackendChoice::Auto);
    assert_eq!(defaults.network, NetworkPolicy::Allow);
    assert!(defaults.writable_paths.is_empty());
    assert!(validate(&defaults).is_ok());
}

#[test]
fn backends_of_another_os_fail_validation() {
    for backend in Backend::ALL {
        let configured = SandboxConfig {
            backend: match backend {
                Backend::Seatbelt => BackendChoice::Seatbelt,
                Backend::Bubblewrap => BackendChoice::Bubblewrap,
                Backend::Landlock => BackendChoice::Landlock,
            },
            ..SandboxConfig::default()
        };
        let result = validate(&configured);
        assert_eq!(
            result.is_ok(),
            backend.supported_here(),
            "{backend:?}: {result:?}"
        );
        if let Err(error) = result {
            assert!(error.to_string().contains(backend.id()), "{error}");
            assert!(error.to_string().contains("not supported"), "{error}");
        }
    }
}

#[test]
fn writable_paths_are_bounded_absolute_and_without_traversal() {
    for good in ["/opt/cache", "~/.cargo", "~", "~/a b/c"] {
        assert!(validate_writable_path(good).is_ok(), "{good}");
    }
    for bad in [
        "relative/dir",
        "./x",
        "",
        "/opt/../etc",
        "~/../other",
        "~user/x",
        "/tmp/\nx",
    ] {
        assert!(validate_writable_path(bad).is_err(), "{bad:?}");
    }
    assert!(validate_writable_path(&format!("/{}", "a".repeat(MAX_PATH_BYTES))).is_err());
    let many = SandboxConfig {
        writable_paths: (0..=MAX_WRITABLE_PATHS)
            .map(|i| format!("/tmp/{i}"))
            .collect(),
        ..SandboxConfig::default()
    };
    assert!(validate(&many).is_err());
}

#[test]
fn project_layers_may_only_tighten() {
    let user = config(r#"{"writable_paths":["~/.cargo","/opt/cache"]}"#);
    let ok =
        |layer: &str| check_restriction(&user, &serde_json::from_str(layer).unwrap(), "project");
    assert!(ok(r#"{"network":"block"}"#).is_ok());
    assert!(ok(r#"{"enabled":true}"#).is_ok());
    assert!(ok(r#"{"backend":"auto"}"#).is_ok());
    assert!(ok(r#"{"writable_paths":["~/.cargo"]}"#).is_ok());
    assert!(ok(r#"{"writable_paths":[]}"#).is_ok());
    let error = ok(r#"{"enabled":false}"#).unwrap_err().to_string();
    assert!(error.contains("cannot disable the sandbox"), "{error}");
    assert!(ok(r#"{"writable_paths":["/etc"]}"#).is_err());
    assert!(ok(r#"{"backend":"landlock"}"#).is_err());
    assert!(ok(r#""off""#).is_err());
    let blocked = config(r#"{"network":"block"}"#);
    assert!(
        check_restriction(&blocked, &serde_json::json!({"network":"allow"}), "project").is_err()
    );
    let off = config(r#"{"enabled":false}"#);
    assert!(check_restriction(&off, &serde_json::json!({"enabled":true}), "project").is_ok());
}

fn probe_from(available: &'static [(Backend, u32)]) -> impl Fn(Backend) -> Availability {
    move |backend| match available.iter().find(|(b, _)| *b == backend) {
        Some((_, abi)) => Availability::yes("works", *abi),
        None => Availability::no("missing"),
    }
}

#[test]
fn auto_never_blocks_and_explicit_backends_fail_closed() {
    let auto = SandboxConfig::default();
    let (enforcement, reason) = choose_with(&auto, probe_from(&[]));
    assert_eq!(enforcement, Enforcement::Unavailable);
    assert!(reason.contains("not available on this system"), "{reason}");
    if cfg!(target_os = "macos") {
        let (enforcement, reason) = choose_with(&auto, probe_from(&[(Backend::Seatbelt, 0)]));
        assert_eq!(enforcement, Enforcement::Active(Backend::Seatbelt));
        assert_eq!(reason, "chosen automatically for macOS");
    }
    if cfg!(target_os = "linux") {
        let (enforcement, _) = choose_with(
            &auto,
            probe_from(&[(Backend::Bubblewrap, 0), (Backend::Landlock, 5)]),
        );
        assert_eq!(enforcement, Enforcement::Active(Backend::Bubblewrap));
        let (enforcement, reason) = choose_with(&auto, probe_from(&[(Backend::Landlock, 3)]));
        assert_eq!(enforcement, Enforcement::Active(Backend::Landlock));
        assert!(reason.contains("bubblewrap: missing"), "{reason}");
        let landlock = config(r#"{"backend":"landlock","network":"block"}"#);
        let (enforcement, _) = choose_with(&landlock, probe_from(&[(Backend::Landlock, 3)]));
        assert!(matches!(
            enforcement,
            Enforcement::FailClosed(Backend::Landlock, _)
        ));
        let (enforcement, _) = choose_with(&landlock, probe_from(&[(Backend::Landlock, 4)]));
        assert_eq!(enforcement, Enforcement::Active(Backend::Landlock));
    }
    let explicit = SandboxConfig {
        backend: BackendChoice::Seatbelt,
        ..SandboxConfig::default()
    };
    let (enforcement, reason) = choose_with(&explicit, probe_from(&[]));
    assert!(
        matches!(enforcement, Enforcement::FailClosed(Backend::Seatbelt, _)),
        "{enforcement:?}"
    );
    assert!(reason.contains("cannot be used"), "{reason}");
    let off = config(r#"{"enabled":false}"#);
    assert_eq!(choose_with(&off, probe_from(&[])).0, Enforcement::Disabled);
}

#[test]
fn fail_closed_plans_refuse_and_name_the_backend() {
    let fx = fixture();
    let plan = plan_with(
        Enforcement::FailClosed(Backend::Bubblewrap, "bwrap is not installed".into()),
        &SandboxConfig::default(),
        &fx.root,
        &fx.app_data,
        &fx.home,
    );
    let refusal = plan.refusal().unwrap();
    assert!(refusal.contains("bubblewrap"), "{refusal}");
    assert!(plan.prepare("echo", &[], None).is_err());
    assert_eq!(plan.status().state, "blocked");
}

#[test]
fn unavailable_and_disabled_plans_run_programs_unchanged() {
    let fx = fixture();
    for enforcement in [Enforcement::Unavailable, Enforcement::Disabled] {
        let plan = plan_with(
            enforcement,
            &SandboxConfig::default(),
            &fx.root,
            &fx.app_data,
            &fx.home,
        );
        let prepared = plan.prepare("echo", &["hi".into()], None).unwrap();
        assert_eq!(prepared.program, "echo");
        assert_eq!(prepared.args, vec![std::ffi::OsString::from("hi")]);
        assert!(plan.refusal().is_none());
        assert!(plan
            .denial(false, "touch: /x: Operation not permitted", &fx.root)
            .is_none());
    }
}

#[test]
fn plan_writable_protected_and_secret_lists() {
    let fx = fixture();
    let configured = config(r#"{"writable_paths":["~/cache"]}"#);
    let plan = plan_with(
        Enforcement::Active(Backend::Seatbelt),
        &configured,
        &fx.root,
        &fx.app_data,
        &fx.home,
    );
    assert_eq!(plan.writable()[0], fx.root);
    assert!(plan.writable().contains(&fx.home.join("cache")));
    assert!(plan
        .writable()
        .contains(&resolve_existing(&std::env::temp_dir())));
    assert_eq!(plan.protected()[0], fx.app_data);
    assert_eq!(plan.protected()[1], fx.root.join(".themis"));
    assert!(plan.secrets().contains(&fx.home.join(".ssh")));
    let status = plan.status();
    assert_eq!(status.state, "on");
    assert!(status.secret_paths.contains(&"~/.ssh".to_owned()));
    assert_eq!(status.configured_writable_paths, vec!["~/cache"]);
}

#[test]
fn a_read_only_plan_keeps_walls_but_writes_only_temp_folders() {
    let fx = fixture();
    let configured = config(r#"{"writable_paths":["~/cache"]}"#);
    let plan = plan_with(
        Enforcement::Active(Backend::Seatbelt),
        &configured,
        &fx.root,
        &fx.app_data,
        &fx.home,
    )
    .read_only();
    assert!(!plan.writable().contains(&fx.root));
    assert!(!plan.writable().contains(&fx.home.join("cache")));
    assert!(plan
        .writable()
        .contains(&resolve_existing(&std::env::temp_dir())));
    assert_eq!(plan.protected()[0], fx.app_data);
    assert!(plan.secrets().contains(&fx.home.join(".ssh")));
    assert_eq!(plan.status().state, "on");
    assert!(!plan.open());
    // In-process file tools are held to the same folders.
    let guard = plan.file_guard(false).unwrap();
    assert!(guard.check_write(&fx.root.join("src/main.rs")).is_err());
    assert!(guard
        .check_write(&std::env::temp_dir().join("scratch.txt"))
        .is_ok());
    // Stepping out of a temp folder is refused, however the path is spelled.
    let escape = std::env::temp_dir()
        .join("..")
        .join(fx.root.strip_prefix("/").unwrap_or(&fx.root));
    assert!(guard.check_write(&escape.join("src/main.rs")).is_err());
    #[cfg(unix)]
    {
        let link = tempfile::tempdir().unwrap();
        let alias = link.path().join("project");
        std::os::unix::fs::symlink(&fx.root, &alias).unwrap();
        assert!(guard.check_write(&alias.join("src/main.rs")).is_err());
    }
    assert!(guard.check_read(&fx.root.join("src/main.rs")).is_ok());

    // A YOLO run's subagent keeps YOLO's reach for reads, and says so in its status.
    let open = SandboxPlan::with_enforcement(
        PlanInputs {
            config: &configured,
            project_root: &fx.root,
            app_data: Some(&fx.app_data),
            home: Some(&fx.home),
            open: Some(YoloSandbox::default()),
        },
        Enforcement::Active(Backend::Seatbelt),
        "test".into(),
    )
    .read_only();
    assert!(open.open());
    assert!(!open.writable().contains(&PathBuf::from("/")));
    assert!(!open.writable().contains(&fx.root));
    assert_eq!(
        open.status().secret_paths.is_empty(),
        open.secrets().is_empty()
    );
}

#[test]
fn file_guard_protects_app_data_secrets_and_yolo_project_config() {
    let fx = fixture();
    let plan = plan_with(
        Enforcement::Unavailable,
        &SandboxConfig::default(),
        &fx.root,
        &fx.app_data,
        &fx.home,
    );
    let custom = plan.file_guard(false).unwrap();
    assert!(custom
        .check_write(&fx.app_data.join("runtime.jsonc"))
        .is_err());
    assert!(custom
        .check_write(&fx.app_data.join("chat_grants/x.json"))
        .is_err());
    assert!(custom.check_read(&fx.home.join(".ssh/id_test")).is_err());
    assert!(custom
        .check_write(&fx.root.join(".themis/config.jsonc"))
        .is_ok());
    assert!(custom.check_write(&fx.root.join("src/main.rs")).is_ok());
    let yolo = plan.file_guard(true).unwrap();
    assert!(yolo
        .check_write(&fx.root.join(".themis/config.jsonc"))
        .is_err());
    assert!(yolo
        .check_read(&fx.root.join(".themis/config.jsonc"))
        .is_ok());
    let off = plan_with(
        Enforcement::Disabled,
        &SandboxConfig::default(),
        &fx.root,
        &fx.app_data,
        &fx.home,
    );
    assert!(off.file_guard(true).is_none());
}

#[cfg(unix)]
#[test]
fn denials_are_recognized_only_for_paths_the_sandbox_blocks() {
    let fx = fixture();
    let plan = plan_with(
        Enforcement::Active(Backend::Seatbelt),
        &SandboxConfig::default(),
        &fx.root,
        &fx.app_data,
        &fx.home,
    );
    let outside = fx.outside.join("x");
    let cases = [
        (
            format!("touch: {}: Operation not permitted", outside.display()),
            true,
        ),
        (
            format!(
                "sh: {}: Operation not permitted",
                fx.app_data.join("runtime.jsonc").display()
            ),
            true,
        ),
        (
            format!(
                "cat: {}: Operation not permitted",
                fx.home.join(".ssh/id_test").display()
            ),
            true,
        ),
        ("cat: ~/.ssh/id_test: Permission denied".to_owned(), true),
        (
            format!(
                "PermissionError: [Errno 1] Operation not permitted: '{}'",
                outside.display()
            ),
            true,
        ),
        // Inside the project: the sandbox allows it, so something else failed.
        (
            format!("touch: {}: Permission denied", fx.root.join("x").display()),
            false,
        ),
        // No path: no evidence.
        ("Error: Operation not permitted".to_owned(), false),
        // An ordinary failure.
        ("error[E0425]: cannot find value `x`".to_owned(), false),
        // A missing file outside secrets is just missing.
        (
            format!("cat: {}: No such file or directory", outside.display()),
            false,
        ),
    ];
    for (output, blocked) in cases {
        assert_eq!(
            plan.denial(false, &output, &fx.root).is_some(),
            blocked,
            "{output}"
        );
    }
    // The path next to the message wins over the program path.
    let named = format!(
        "/usr/bin/sh: 1: cannot create {}: Read-only file system",
        outside.display()
    );
    assert!(
        plan.denial(false, &named, &fx.root)
            .unwrap()
            .contains("outside"),
        "{named}"
    );
    // Hidden secret folders look missing under bubblewrap.
    let hidden = format!(
        "cat: {}: No such file or directory",
        fx.home.join(".ssh/id_test").display()
    );
    assert!(plan.denial(false, &hidden, &fx.root).is_some());
    // Relative paths resolve against the working folder.
    let relative = "cat: ../home/.ssh/id_test: Operation not permitted";
    assert!(plan.denial(false, relative, &fx.root).is_some());
    // Successful commands are never denials.
    assert!(plan
        .denial(
            true,
            &format!("{}: Operation not permitted", outside.display()),
            &fx.root
        )
        .is_none());
    // The OS itself refuses writing here, so it is not the sandbox.
    #[cfg(unix)]
    if unsafe { libc::geteuid() } != 0 {
        assert!(plan
            .denial(false, "sh: /etc/hosts: Permission denied", &fx.root)
            .is_none());
    }
}

#[cfg(unix)]
#[test]
fn network_failures_are_denials_only_when_the_network_is_blocked() {
    let fx = fixture();
    let output = "curl: (7) Failed to connect to example.com port 443: Couldn't connect to server";
    let allow = plan_with(
        Enforcement::Active(Backend::Seatbelt),
        &SandboxConfig::default(),
        &fx.root,
        &fx.app_data,
        &fx.home,
    );
    assert!(allow.denial(false, output, &fx.root).is_none());
    let block = plan_with(
        Enforcement::Active(Backend::Seatbelt),
        &config(r#"{"network":"block"}"#),
        &fx.root,
        &fx.app_data,
        &fx.home,
    );
    assert!(block
        .denial(false, output, &fx.root)
        .unwrap()
        .contains("network"));
}

#[test]
fn seatbelt_profile_keeps_paths_out_of_the_profile_text() {
    let writable = vec![
        PathBuf::from("/w/a\" (allow default)"),
        PathBuf::from("/w/b"),
    ];
    let protected = vec![PathBuf::from("/p/)\n(allow file-write*")];
    let secrets = vec![PathBuf::from("/s/#\"x\"")];
    let (text, params) = seatbelt::build(&writable, &protected, &secrets, true);
    assert_eq!(params.len(), 4);
    assert!(!text.contains("/w/"));
    assert!(text.contains("(deny network-outbound (remote ip))"));
    assert!(text.contains("(deny file-read* file-write*"));
    let args = seatbelt::arguments(&text, &params, Path::new("/bin/echo"), &["x".into()]);
    assert_eq!(args[0], "-p");
    assert!(args.contains(&std::ffi::OsString::from(
        "WRITABLE_0=/w/a\" (allow default)"
    )));
    let separator = args.iter().position(|arg| arg == "--").unwrap();
    assert_eq!(args[separator + 1], "/bin/echo");
    assert_eq!(args[separator + 2], "x");
}

#[test]
fn bubblewrap_binds_writable_then_protects_then_hides_secrets() {
    let fx = fixture();
    let args: Vec<String> = bubblewrap::build(
        std::slice::from_ref(&fx.root),
        &[fx.app_data.clone(), fx.root.join(".themis-missing")],
        &[fx.home.join(".ssh"), fx.home.join(".netrc-missing")],
        true,
    )
    .iter()
    .map(|arg| arg.to_string_lossy().into_owned())
    .collect();
    let position = |needle: &str| args.iter().position(|arg| arg == needle).unwrap();
    assert_eq!(&args[..5], ["--ro-bind", "/", "/", "--dev", "/dev"]);
    assert!(position("--bind") < position(&fx.app_data.display().to_string()));
    assert!(args.contains(&"--tmpfs".to_owned()));
    assert!(args.contains(&"--unshare-net".to_owned()));
    assert!(!args.iter().any(|arg| arg.contains("missing")), "{args:?}");
    assert_eq!(args.last().unwrap(), "--die-with-parent");
}

#[test]
fn splitting_grants_everything_but_the_excluded_subtrees() {
    let fx = fixture();
    let home = &fx.home;
    std::fs::create_dir_all(home.join("code/app")).unwrap();
    std::fs::create_dir_all(home.join(".config/gh")).unwrap();
    std::fs::create_dir_all(home.join(".config/other")).unwrap();
    std::fs::write(home.join("notes.txt"), "x").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(home.join(".ssh"), home.join("keys")).unwrap();
        std::os::unix::fs::symlink(home.join("code"), home.join("work")).unwrap();
    }
    let excluded = vec![home.join(".ssh"), home.join(".config/gh")];
    let mut budget = 1000;
    let (full, list_only) = split_around(home, &excluded, &mut budget).unwrap();
    assert!(list_only.contains(home));
    assert!(list_only.contains(&home.join(".config")));
    assert!(full.contains(&home.join("code")));
    assert!(full.contains(&home.join("notes.txt")));
    assert!(full.contains(&home.join(".config/other")));
    for path in full.iter().chain(&list_only) {
        assert!(!excluded.iter().any(|ex| path.starts_with(ex)), "{path:?}");
    }
    #[cfg(unix)]
    assert!(!full.contains(&home.join("keys")) && !full.iter().any(|p| p.ends_with(".ssh")));
    let mut tiny = 1;
    assert!(split_around(home, &excluded, &mut tiny).is_err());
    let mut budget = 10;
    let (full, list_only) = split_around(&home.join("code"), &excluded, &mut budget).unwrap();
    assert_eq!(full, vec![home.join("code")]);
    assert!(list_only.is_empty());
}

#[test]
fn programs_are_found_on_the_given_path_only() {
    let found = find_program("sh", std::env::var_os("PATH").as_deref());
    if cfg!(unix) {
        assert!(found.is_some());
    }
    assert!(find_program(
        "definitely-not-a-program-xyz",
        std::env::var_os("PATH").as_deref()
    )
    .is_none());
    assert!(find_program("", None).is_none());
    assert!(find_program("sh", None).is_none());
}

proptest! {
    #[test]
    fn seatbelt_profile_never_contains_path_text(
        paths in proptest::collection::vec("/[^\u{0}]{1,40}", 1..6),
        block in proptest::bool::ANY,
    ) {
        let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        let (text, params) = seatbelt::build(&paths, &paths, &paths, block);
        prop_assert_eq!(params.len(), paths.len() * 3);
        let fixed = seatbelt::build::<PathBuf>(&[], &[], &[], block).0;
        // Only fixed rule lines and `(param "NAME")` references are added.
        for line in text.lines() {
            prop_assert!(
                fixed.lines().any(|known| known == line)
                    || line.starts_with("  (subpath (param \"")
                    || line == "(deny file-write*"
                    || line == "(deny file-read* file-write*"
                    || line == ")",
                "unexpected profile line {:?}", line
            );
        }
        for (index, (name, value)) in params.iter().enumerate() {
            prop_assert!(name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'));
            prop_assert_eq!(value, paths[index % paths.len()].as_os_str());
        }
    }

    #[test]
    fn path_candidates_are_bounded_substrings(line in ".{0,300}") {
        let found = detect::path_candidates(&line);
        prop_assert!(found.len() <= 16);
        for candidate in found {
            prop_assert!(!candidate.is_empty());
            prop_assert!(line.contains(&candidate));
        }
    }

    #[test]
    fn denial_detection_never_panics(output in ".{0,400}", success in proptest::bool::ANY) {
        let root = std::env::temp_dir();
        let home = root.join("themis-no-home");
        let plan = SandboxPlan::with_enforcement(
            PlanInputs { config: &SandboxConfig::default(), project_root: &root, app_data: None, home: Some(&home), open: None },
            Enforcement::Active(Backend::Seatbelt),
            "test".into(),
        );
        let _ = plan.denial(success, &output, &root);
        let _ = plan.may_have_blocked(success, &output);
    }

    #[test]
    fn restriction_never_widens(
        enabled in proptest::option::of(proptest::bool::ANY),
        network in proptest::option::of(proptest::sample::select(vec!["allow", "block"])),
        extra in proptest::collection::vec("/[a-z]{1,6}", 0..3),
    ) {
        let user = config(r#"{"network":"block","writable_paths":["/keep"]}"#);
        let mut layer = serde_json::Map::new();
        if let Some(enabled) = enabled { layer.insert("enabled".into(), enabled.into()); }
        if let Some(network) = network { layer.insert("network".into(), network.into()); }
        if !extra.is_empty() { layer.insert("writable_paths".into(), serde_json::json!(extra)); }
        let accepted = check_restriction(&user, &serde_json::Value::Object(layer), "project").is_ok();
        let widens = enabled == Some(false)
            || network == Some("allow")
            || extra.iter().any(|path| path != "/keep");
        prop_assert_eq!(accepted, !widens);
    }
}

/// Runs `sh -c script` through `plan` and returns (success, output).
#[cfg(unix)]
fn run(plan: &SandboxPlan, cwd: &Path, script: &str) -> (bool, String) {
    let prepared = plan
        .prepare(
            "sh",
            &["-c".into(), script.into()],
            std::env::var_os("PATH").as_deref(),
        )
        .unwrap();
    let output = prepared.command().current_dir(cwd).output().unwrap();
    (
        output.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

/// The real backend on this machine confines writes, secrets and the app-data
/// folder; skipped with a reason when the machine has none.
#[cfg(unix)]
fn check_real_backend(backend: Backend) {
    let state = availability(backend);
    if !state.available {
        eprintln!(
            "skipping {} enforcement test: {}",
            backend.label(),
            state.detail
        );
        return;
    }
    let fx = fixture();
    let plan = plan_with(
        Enforcement::Active(backend),
        &SandboxConfig::default(),
        &fx.root,
        &fx.app_data,
        &fx.home,
    );
    let (ok, output) = run(&plan, &fx.root, "echo hi > inside.txt && cat inside.txt");
    assert!(ok, "{output}");
    assert!(fx.root.join("inside.txt").exists());
    let temp = std::env::temp_dir().join(format!("themis-sandbox-{}", std::process::id()));
    let (ok, output) = run(
        &plan,
        &fx.root,
        &format!("echo t > '{}' && rm '{}'", temp.display(), temp.display()),
    );
    assert!(ok, "temp folders stay writable: {output}");
    let outside = fx.outside.join("x.txt");
    let (ok, output) = run(
        &plan,
        &fx.root,
        &format!("echo x > '{}'", outside.display()),
    );
    assert!(!ok && !outside.exists(), "{output}");
    assert!(plan.denial(ok, &output, &fx.root).is_some(), "{output}");
    let secret = fx.home.join(".ssh/id_test");
    let (ok, output) = run(&plan, &fx.root, &format!("cat '{}'", secret.display()));
    assert!(!ok && !output.contains("secret"), "{output}");
    assert!(plan.denial(ok, &output, &fx.root).is_some(), "{output}");
    let config_file = fx.app_data.join("runtime.jsonc");
    let (ok, output) = run(
        &plan,
        &fx.root,
        &format!("echo '{{\"x\":1}}' > '{}'", config_file.display()),
    );
    assert!(!ok, "{output}");
    assert_eq!(std::fs::read_to_string(&config_file).unwrap(), "{}");
    let grants = fx.app_data.join("chat_grants/new.json");
    let (ok, output) = run(&plan, &fx.root, &format!("echo x > '{}'", grants.display()));
    assert!(!ok && !grants.exists(), "{output}");
    let (ok, output) = run(
        &plan,
        &fx.root,
        "cat /etc/hosts > /dev/null && echo read-ok",
    );
    assert!(
        ok && output.contains("read-ok"),
        "reads stay allowed: {output}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn seatbelt_confines_a_real_command() {
    check_real_backend(Backend::Seatbelt);
}

#[cfg(target_os = "linux")]
#[test]
fn bubblewrap_confines_a_real_command() {
    check_real_backend(Backend::Bubblewrap);
}

#[cfg(target_os = "linux")]
#[test]
fn landlock_confines_a_real_command() {
    check_real_backend(Backend::Landlock);
}

/// Network policy on the real backend, against a listener on this machine.
#[cfg(unix)]
fn check_real_network(backend: Backend) {
    let state = availability(backend);
    if !state.available || (backend == Backend::Landlock && state.abi < 4) {
        eprintln!(
            "skipping {} network test: {}",
            backend.label(),
            state.detail
        );
        return;
    }
    if find_program("curl", std::env::var_os("PATH").as_deref()).is_none() {
        eprintln!(
            "skipping {} network test: curl is not installed",
            backend.label()
        );
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().take(2).flatten() {
            use std::io::{Read, Write};
            let mut buffer = [0u8; 1024];
            let _ = stream.read(&mut buffer);
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        }
    });
    let fx = fixture();
    let script = format!("curl -sS -m 5 http://127.0.0.1:{port}/");
    let allow = plan_with(
        Enforcement::Active(backend),
        &SandboxConfig::default(),
        &fx.root,
        &fx.app_data,
        &fx.home,
    );
    let (ok, output) = run(&allow, &fx.root, &script);
    assert!(
        ok && output.contains("ok"),
        "network is allowed by default: {output}"
    );
    let block = plan_with(
        Enforcement::Active(backend),
        &config(r#"{"network":"block"}"#),
        &fx.root,
        &fx.app_data,
        &fx.home,
    );
    let (ok, output) = run(&block, &fx.root, &script);
    assert!(!ok, "network is blocked: {output}");
    assert!(block.denial(ok, &output, &fx.root).is_some(), "{output}");
}

#[cfg(target_os = "macos")]
#[test]
fn seatbelt_network_policy_on_a_real_command() {
    check_real_network(Backend::Seatbelt);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_network_policy_on_a_real_command() {
    check_real_network(Backend::Bubblewrap);
    check_real_network(Backend::Landlock);
}

#[cfg(windows)]
#[test]
fn windows_has_no_backend_and_runs_unsandboxed() {
    let (enforcement, reason) = choose(&SandboxConfig::default());
    assert_eq!(enforcement, Enforcement::Unavailable);
    assert!(reason.contains("not available on this system"), "{reason}");
    for backend in Backend::ALL {
        assert!(!availability(backend).available);
    }
}

#[test]
fn the_keychain_is_blocked_unless_the_user_allows_it() {
    let fx = fixture();
    let profile = |json: &str| {
        let plan = plan_with(
            Enforcement::Active(Backend::Seatbelt),
            &config(json),
            &fx.root,
            &fx.app_data,
            &fx.home,
        );
        seatbelt::profile(&plan).0
    };
    let blocked = profile("{}");
    for service in [
        "com.apple.SecurityServer",
        "com.apple.securityd",
        "com.apple.securityd.xpc",
    ] {
        assert!(
            blocked.contains(&format!("(deny mach-lookup (global-name \"{service}\"))")),
            "{blocked}"
        );
    }
    assert!(!profile(r#"{"allow_keychain": true}"#).contains("mach-lookup"));
}

#[test]
fn only_the_user_configuration_can_allow_the_keychain() {
    let user = SandboxConfig::default();
    let project = |value: serde_json::Value| check_restriction(&user, &value, "project");
    assert!(project(serde_json::json!({"allow_keychain": false})).is_ok());
    let error = project(serde_json::json!({"allow_keychain": true})).unwrap_err();
    assert!(
        error.to_string().contains("sandbox.allow_keychain"),
        "{error}"
    );
    let allowed = config(r#"{"allow_keychain": true}"#);
    assert!(check_restriction(
        &allowed,
        &serde_json::json!({"allow_keychain": true}),
        "project"
    )
    .is_ok());
}

#[test]
fn keychain_failures_name_the_setting_only_where_it_applies() {
    let fx = fixture();
    let plan = |enforcement, json: &str| {
        plan_with(enforcement, &config(json), &fx.root, &fx.app_data, &fx.home)
    };
    let output = "security: SecKeychainSearchCopyNext: The specified item could not be found in the keychain.";
    let seatbelt = plan(Enforcement::Active(Backend::Seatbelt), "{}");
    let note = seatbelt.denial(false, output, &fx.root).unwrap();
    assert!(note.contains("sandbox.allow_keychain"), "{note}");
    assert!(seatbelt.may_have_blocked(false, output));
    let allowed = plan(
        Enforcement::Active(Backend::Seatbelt),
        r#"{"allow_keychain": true}"#,
    );
    assert!(allowed.denial(false, output, &fx.root).is_none());
    assert!(
        seatbelt.denial(true, output, &fx.root).is_none(),
        "only failed commands"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn seatbelt_keeps_a_real_command_out_of_the_keychain() {
    if !availability(Backend::Seatbelt).available {
        eprintln!("skipping: Seatbelt unavailable");
        return;
    }
    let fx = fixture();
    let plan = |json: &str| {
        plan_with(
            Enforcement::Active(Backend::Seatbelt),
            &config(json),
            &fx.root,
            &fx.app_data,
            &fx.home,
        )
    };
    // Listing the keychain search list needs the Keychain service but no item, so it never
    // raises a prompt.
    let (ok, output) = run(&plan("{}"), &fx.root, "/usr/bin/security list-keychains");
    assert!(!ok || !output.contains("login.keychain"), "{output}");
    let (ok, output) = run(
        &plan(r#"{"allow_keychain": true}"#),
        &fx.root,
        "/usr/bin/security list-keychains",
    );
    assert!(ok, "allowed: {output}");
}

fn open_plan(enforcement: Enforcement, config: &SandboxConfig, fx: &Fixture) -> SandboxPlan {
    SandboxPlan::with_enforcement(
        PlanInputs {
            config,
            project_root: &fx.root,
            app_data: Some(&fx.app_data),
            home: Some(&fx.home),
            open: Some(YoloSandbox::default()),
        },
        enforcement,
        "test".into(),
    )
}

/// YOLO's plan: writable everywhere, no secret locations, network and Keychain on
/// whatever the configuration says; only the app data and `.themis/` stay read-only.
#[test]
fn the_open_plan_writes_everywhere_but_the_protected_folders() {
    let fx = fixture();
    let blocked = config(r#"{"network":"block","allow_keychain":false}"#);
    let plan = open_plan(Enforcement::Active(Backend::Seatbelt), &blocked, &fx);
    assert!(plan.open());
    assert_eq!(plan.writable()[0], PathBuf::from("/"));
    assert!(plan.secrets().is_empty());
    assert_eq!(plan.network(), NetworkPolicy::Allow);
    assert!(plan.allow_keychain());
    assert_eq!(
        plan.protected(),
        [fx.app_data.clone(), fx.root.join(".themis")]
    );
    let (profile, params) = seatbelt::profile(&plan);
    assert!(params
        .iter()
        .any(|(name, path)| name == "WRITABLE_0" && path == "/"));
    assert!(
        profile.contains("(deny file-write*\n  (subpath (param \"PROTECTED_0\"))"),
        "{profile}"
    );
    assert!(
        !profile.contains("SECRET")
            && !profile.contains("network-outbound")
            && !profile.contains("mach-lookup"),
        "{profile}"
    );
    // bubblewrap is Linux-only; elsewhere `/` does not canonicalize to itself.
    if cfg!(unix) {
        let args: Vec<String> = bubblewrap::arguments(&plan)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let bind_root = args
            .windows(3)
            .position(|w| w == ["--bind", "/", "/"])
            .expect("/ is writable");
        let app = fx.app_data.display().to_string();
        let protect = args
            .windows(3)
            .position(|w| w[0] == "--ro-bind" && w[1] == app)
            .expect("app data read-only");
        assert!(
            protect > bind_root,
            "the protected folders are bound read-only over /"
        );
        assert!(
            !args.iter().any(|a| a == "--unshare-net" || a == "--tmpfs"),
            "{args:?}"
        );
    }
    let guard = plan.file_guard(true).unwrap();
    assert!(
        guard.check_read(&fx.home.join(".ssh/id_test")).is_ok(),
        "reads stay open"
    );
    assert!(guard
        .check_write(&fx.app_data.join("runtime.jsonc"))
        .is_err());
    assert!(guard
        .check_write(&fx.root.join(".themis/config.jsonc"))
        .is_err());
    assert!(guard.check_write(&fx.outside.join("x.txt")).is_ok());
    assert!(plan.status().secret_paths.is_empty() && plan.status().open);
    // Blocks a person wrote hold: YOLO removes Themis' default walls, not theirs.
    let written = SandboxPlan::with_enforcement(
        PlanInputs {
            config: &blocked,
            project_root: &fx.root,
            app_data: Some(&fx.app_data),
            home: Some(&fx.home),
            open: Some(YoloSandbox {
                network_written: true,
                keychain_written: true,
            }),
        },
        Enforcement::Active(Backend::Seatbelt),
        "test".into(),
    );
    assert_eq!(written.network(), NetworkPolicy::Block);
    assert!(!written.allow_keychain());
    assert!(
        written.secrets().is_empty(),
        "secret locations stay readable"
    );
}

/// The open plan on the real backend: writes outside the project and reads of secret
/// locations work, the app data stays read-only (and `.themis/` too, except under
/// Landlock, which cannot carve it out; the file tools cover it there).
#[cfg(unix)]
fn check_real_open(backend: Backend) {
    let state = availability(backend);
    if !state.available {
        eprintln!(
            "skipping {} open-plan test: {}",
            backend.label(),
            state.detail
        );
        return;
    }
    let fx = fixture();
    std::fs::create_dir_all(fx.root.join(".themis")).unwrap();
    let plan = open_plan(
        Enforcement::Active(backend),
        &config(r#"{"network":"block"}"#),
        &fx,
    );
    let outside = fx.outside.join("x.txt");
    let (ok, output) = run(
        &plan,
        &fx.root,
        &format!("echo x > '{}'", outside.display()),
    );
    assert!(
        ok && outside.exists(),
        "writes outside the project work: {output}"
    );
    let secret = fx.home.join(".ssh/id_test");
    let (ok, output) = run(&plan, &fx.root, &format!("cat '{}'", secret.display()));
    assert!(
        ok && output.contains("secret"),
        "secret locations are readable: {output}"
    );
    let config_file = fx.app_data.join("runtime.jsonc");
    let (ok, output) = run(
        &plan,
        &fx.root,
        &format!("echo '{{\"x\":1}}' > '{}'", config_file.display()),
    );
    assert!(!ok, "{output}");
    assert_eq!(std::fs::read_to_string(&config_file).unwrap(), "{}");
    assert!(plan.denial(ok, &output, &fx.root).is_some(), "{output}");
    if backend != Backend::Landlock {
        let local = fx.root.join(".themis/config.jsonc");
        let (ok, output) = run(&plan, &fx.root, &format!("echo x > '{}'", local.display()));
        assert!(!ok && !local.exists(), "{output}");
    }
}

#[cfg(target_os = "macos")]
#[test]
fn seatbelt_open_plan_on_a_real_command() {
    check_real_open(Backend::Seatbelt);
}

#[cfg(target_os = "linux")]
#[test]
fn bubblewrap_open_plan_on_a_real_command() {
    check_real_open(Backend::Bubblewrap);
}

#[cfg(target_os = "linux")]
#[test]
fn landlock_open_plan_on_a_real_command() {
    check_real_open(Backend::Landlock);
}
