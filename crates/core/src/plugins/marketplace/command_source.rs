//! Catalog entries whose source is a command that prints where the plugin is.
//!
//! A catalog is downloaded, so its text never chooses what runs: only a command whose exact
//! text is listed here is run, as the listed program and arguments with no shell. Apple's
//! `xcode` plugin ships inside Xcode and is found this way.

use super::*;

/// The catalog commands Themis runs: their exact text, and the program and arguments run.
pub(super) const ALLOWED_COMMANDS: &[(&str, &[&str])] = &[(
    "xcrun agent plugin path --plugin-format claude",
    &[
        "xcrun",
        "agent",
        "plugin",
        "path",
        "--plugin-format",
        "claude",
    ],
)];

/// How long a catalog command may take before it is stopped.
pub(super) const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// More output than one folder path needs means it is not one.
const MAX_OUTPUT_BYTES: usize = 4096;

/// The folder an allowed catalog command prints: one absolute path to an existing folder.
pub(super) async fn command_source_directory(
    source: &Value,
    allowed: &[(&str, &[&str])],
    timeout: std::time::Duration,
) -> anyhow::Result<PathBuf> {
    let text = source["command"].as_str().context("Missing command")?;
    let Some((_, argv)) = allowed.iter().find(|(listed, _)| *listed == text) else {
        bail!("Marketplace command is not one Themis runs");
    };
    let (program, arguments) = argv.split_first().context("Empty marketplace command")?;
    let mut command = tokio::process::Command::new(program);
    command.args(arguments).env_clear();
    for name in ["PATH", "HOME", "TMPDIR", "DEVELOPER_DIR"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| anyhow::anyhow!("Marketplace command did not finish in time"))?
        .with_context(|| format!("Marketplace command could not start ({program})"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason: String = stderr
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(200)
            .collect();
        bail!("Marketplace command failed: {reason}");
    }
    printed_folder(&output.stdout)
}

fn printed_folder(stdout: &[u8]) -> anyhow::Result<PathBuf> {
    let printed = std::str::from_utf8(stdout)
        .ok()
        .filter(|_| stdout.len() <= MAX_OUTPUT_BYTES)
        .map(str::trim)
        .filter(|text| !text.is_empty() && text.lines().count() == 1)
        .context("Marketplace command did not print one folder path")?;
    let path = Path::new(printed);
    if !path.is_absolute() {
        bail!("Marketplace command did not print an absolute folder path");
    }
    let directory = path
        .canonicalize()
        .context("Folder from marketplace command does not exist")?;
    if !directory.is_dir() {
        bail!("Marketplace command printed a file, not a folder");
    }
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn allow(
        text: &'static str,
        argv: Vec<String>,
    ) -> Vec<(&'static str, &'static [&'static str])> {
        let argv: Vec<&'static str> = argv
            .into_iter()
            .map(|part| &*Box::leak(part.into_boxed_str()))
            .collect();
        vec![(text, Box::leak(argv.into_boxed_slice()))]
    }

    fn shell(script: &str) -> Vec<String> {
        vec!["/bin/sh".into(), "-c".into(), script.into()]
    }

    async fn resolve(
        command: &str,
        allowed: &[(&str, &[&str])],
        timeout: Duration,
    ) -> anyhow::Result<PathBuf> {
        command_source_directory(
            &json!({"source": "command", "command": command}),
            allowed,
            timeout,
        )
        .await
    }

    #[tokio::test]
    async fn a_command_off_the_allowlist_is_refused_and_never_run() {
        let marker = tempfile::tempdir().unwrap();
        let touched = marker.path().join("ran");
        let command = format!("touch {}", touched.display());
        let error = resolve(&command, ALLOWED_COMMANDS, Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not one Themis runs"), "{error}");
        assert!(!touched.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_allowed_command_names_the_folder_it_prints() {
        let plugin = tempfile::tempdir().unwrap();
        let script = format!("printf '%s\\n' '{}'", plugin.path().display());
        let allowed = allow("find plugin", shell(&script));
        let directory = resolve("find plugin", &allowed, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(directory, plugin.path().canonicalize().unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failing_command_says_why() {
        let allowed = allow(
            "find plugin",
            shell(
                "echo 'xcrun: error: unable to find utility \"agent\"' >&2; echo more >&2; exit 72",
            ),
        );
        let error = resolve("find plugin", &allowed, Duration::from_secs(5))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("unable to find utility"), "{error}");
        assert!(!error.contains("more"), "only the first line: {error}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_that_is_not_one_existing_absolute_folder_is_refused() {
        let file = tempfile::NamedTempFile::new().unwrap();
        for script in [
            "echo relative/plugin".to_owned(),
            "printf '/tmp\\n/var\\n'".to_owned(),
            "echo /no/such/folder/for/themis".to_owned(),
            format!("echo {}", file.path().display()),
            String::new(),
        ] {
            let allowed = allow("find plugin", shell(&script));
            let error = resolve("find plugin", &allowed, Duration::from_secs(5)).await;
            assert!(error.is_err(), "{script:?} gave {error:?}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_that_hangs_is_stopped() {
        let allowed = allow("find plugin", shell("sleep 30"));
        let started = std::time::Instant::now();
        let error = resolve("find plugin", &allowed, Duration::from_millis(300))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("did not finish"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn only_xcodes_plugin_lookup_is_allowed() {
        assert_eq!(
            ALLOWED_COMMANDS,
            &[(
                "xcrun agent plugin path --plugin-format claude",
                &[
                    "xcrun",
                    "agent",
                    "plugin",
                    "path",
                    "--plugin-format",
                    "claude"
                ][..],
            )]
        );
    }
}
