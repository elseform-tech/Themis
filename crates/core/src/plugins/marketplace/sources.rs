use super::*;

/// The stored form of what a person typed: `owner/repo` becomes a GitHub URL, and a `~` path an
/// absolute one.
pub(super) fn normalize_source(input: &str) -> anyhow::Result<String> {
    let input = input.trim();
    let is_part = |part: &str| {
        !part.is_empty()
            && !part.starts_with('.')
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    };
    let source = match input.split_once('/') {
        Some((owner, repo)) if is_part(owner) && is_part(repo) => {
            format!("https://github.com/{owner}/{repo}.git")
        }
        _ => match input.strip_prefix("~/").zip(std::env::var_os("HOME")) {
            Some((rest, home)) => Path::new(&home).join(rest).to_string_lossy().into_owned(),
            None => input.to_owned(),
        },
    };
    validate_source(&source)?;
    Ok(source)
}
/// Fetches only `.claude-plugin/marketplace.json` into `staging`: shallow, sparse and without
/// other file contents, so previewing a large repository stays cheap.
pub(super) async fn read_git_manifest(source: &str, staging: &Path) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(staging.parent().context("Missing cache parent")?)?;
    let path = staging.to_str().context("Invalid cache path")?;
    let unreachable = "This source can't be reached.";
    run_git(
        &[
            "clone",
            "--depth=1",
            "--filter=blob:none",
            "--no-checkout",
            "--sparse",
            "--",
            source,
            path,
        ],
        None,
    )
    .await
    .context(unreachable)?;
    run_git(
        &[
            "sparse-checkout",
            "set",
            "--no-cone",
            "/.claude-plugin/marketplace.json",
        ],
        Some(staging),
    )
    .await
    .context(unreachable)?;
    run_git(&["checkout"], Some(staging))
        .await
        .context(unreachable)?;
    Ok(staging.to_path_buf())
}
/// What the manifest in `directory` says about a source, or the one-line reason it isn't one.
pub(super) fn source_preview(source: &str, directory: &Path) -> anyhow::Result<SourcePreview> {
    let path = directory.join(".claude-plugin/marketplace.json");
    let Ok(metadata) = fs::metadata(&path) else {
        bail!("No marketplace found in this source.");
    };
    if metadata.len() > 8 * 1024 * 1024 {
        bail!("This marketplace's catalog is too large.");
    }
    let value: Value = serde_json::from_str(&fs::read_to_string(&path)?)
        .ok()
        .filter(|value: &Value| value["plugins"].is_array())
        .context("No marketplace found in this source.")?;
    let plugins = value["plugins"].as_array().map_or(0, Vec::len);
    if plugins > 2000 {
        bail!("This marketplace has more than 2,000 plugins.");
    }
    // Catalogs may leave out their name; the source's last path part then names them.
    let folder = source
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    let name = value["name"].as_str().unwrap_or(folder);
    if !safe_name(name) {
        bail!("This marketplace's name isn't valid.");
    }
    let publisher = ["owner", "author"]
        .iter()
        .find_map(|key| value[key]["name"].as_str().or_else(|| value[key].as_str()))
        .unwrap_or_default();
    Ok(SourcePreview {
        name: name.into(),
        publisher: publisher.into(),
        plugins,
        source: source.into(),
    })
}
pub(super) fn validate_source(source: &str) -> anyhow::Result<()> {
    if source.starts_with("https://") {
        let url = reqwest::Url::parse(source)?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            bail!("Use a repository URL without credentials, query, or fragment");
        }
    } else if !Path::new(source).is_absolute() {
        bail!("Use an HTTPS repository URL or absolute local directory");
    }
    Ok(())
}
pub(super) async fn clone_repo(source: &str, path: &Path) -> anyhow::Result<()> {
    clone_repository(source, path, None).await
}
pub(super) async fn clone_repository(
    source: &str,
    path: &Path,
    reference: Option<&str>,
) -> anyhow::Result<()> {
    if reference.is_some_and(|value| {
        value.is_empty() || value.starts_with('-') || value.chars().any(char::is_control)
    }) {
        bail!("Invalid repository reference");
    }
    fs::create_dir_all(path.parent().context("Missing cache parent")?)?;
    let limit = super::package::MAX_CLONE_BYTES;
    run_git_within(
        &[
            "clone",
            "--depth=1",
            "--",
            source,
            path.to_str().context("Invalid cache path")?,
        ],
        None,
        Some((path, limit)),
    )
    .await?;
    if let Some(reference) = reference {
        let fetch = ["fetch", "--depth=1", "origin", reference];
        run_git_within(&fetch, Some(path), Some((path, limit))).await?;
        let checkout = ["checkout", "--detach", "FETCH_HEAD"];
        run_git_within(&checkout, Some(path), Some((path, limit))).await?;
    }
    Ok(())
}
/// The commit a remote reference points at, without downloading anything.
pub(crate) async fn git_remote_ref(url: &str, reference: Option<&str>) -> anyhow::Result<String> {
    let reference = reference.unwrap_or("HEAD");
    if reference.starts_with('-') || reference.chars().any(char::is_control) {
        bail!("Invalid repository reference");
    }
    let mut command = tokio::process::Command::new("git");
    command
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "ls-remote",
            "--",
            url,
            reference,
        ])
        .env_clear();
    for name in ["PATH", "HOME", "SYSTEMROOT"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        command
            .env("GIT_TERMINAL_PROMPT", "0")
            .kill_on_drop(true)
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await
    .context("Repository check timed out")??;
    if !output.status.success() {
        bail!("Repository check failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned())
}
/// A bounded hash of a folder's files: names, sizes and contents, not modification times. Past
/// the bound only names and sizes count.
pub(crate) fn tree_fingerprint(root: &Path) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    const FILES: usize = 5000;
    const CONTENT_BYTES: u64 = 64 * 1024 * 1024;
    let mut hash = Sha256::new();
    let mut stack = vec![root.to_path_buf()];
    let (mut files, mut read) = (0, 0u64);
    while let Some(directory) = stack.pop() {
        let mut entries = fs::read_dir(&directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                stack.push(path);
                continue;
            }
            files += 1;
            if files > FILES {
                return Ok(format!("{:x}", hash.finalize()));
            }
            hash.update(path.strip_prefix(root)?.to_string_lossy().as_bytes());
            hash.update(metadata.len().to_le_bytes());
            if metadata.is_symlink() {
                hash.update(fs::read_link(&path)?.to_string_lossy().as_bytes());
            } else if read + metadata.len() <= CONTENT_BYTES {
                read += metadata.len();
                hash.update(fs::read(&path)?);
            }
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}
async fn run_git(args: &[&str], directory: Option<&Path>) -> anyhow::Result<()> {
    run_git_within(args, directory, None).await
}
/// How long one git command may take: a download of a large repository on a slow line.
const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);
/// How often a download's size is looked at.
const CLONE_WATCH: std::time::Duration = std::time::Duration::from_secs(2);
/// Runs git, stopping it when the folder in `watch` grows past its limit: a repository can't
/// fill the disk before the time limit is up.
pub(super) async fn run_git_within(
    args: &[&str],
    directory: Option<&Path>,
    watch: Option<(&Path, u64)>,
) -> anyhow::Result<()> {
    let mut command = tokio::process::Command::new("git");
    command
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .env_clear();
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    for name in ["PATH", "HOME", "SYSTEMROOT"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let too_large = || watch.is_some_and(|(path, limit)| super::package::tree_bytes(path) > limit);
    let mut child = command.spawn()?;
    let status = tokio::time::timeout(GIT_TIMEOUT, async {
        loop {
            tokio::select! {
                status = child.wait() => return status.map_err(anyhow::Error::from),
                () = tokio::time::sleep(CLONE_WATCH) => {
                    if too_large() {
                        let _ = child.kill().await;
                        bail!(REPOSITORY_TOO_LARGE);
                    }
                }
            }
        }
    })
    .await
    .context("Repository download timed out")??;
    if !status.success() {
        bail!("Repository download failed; check repository access, reference, and network");
    }
    // One that finished between two looks.
    if too_large() {
        bail!(REPOSITORY_TOO_LARGE);
    }
    Ok(())
}
pub(super) const REPOSITORY_TOO_LARGE: &str = "Repository is too large to download (over 2 GiB)";
