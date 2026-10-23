//! The built-in browser engine: Google's `chrome-headless-shell` (Chrome for Testing),
//! downloaded into app data the first time the browser is used. It is a plain program,
//! not an app bundle, so macOS never shows it in the Dock (a headless Google Chrome
//! appears there under recent apps), and the browser works without Chrome installed.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context};
use serde_json::Value;

/// Google's list of current Chrome for Testing builds and their downloads.
const VERSIONS_URL: &str =
    "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json";
/// Downloads are only taken from Google's Chrome for Testing bucket.
const DOWNLOAD_PREFIX: &str = "https://storage.googleapis.com/chrome-for-testing-public/";
/// The archives are about 100–120 MB; anything far larger is not the engine.
const MAX_DOWNLOAD: u64 = 400 * 1024 * 1024;
/// The unpacked engine is about 200–300 MB.
const MAX_UNPACKED: u64 = 3 * MAX_DOWNLOAD;
/// A slow connection gets time to finish; a stalled one does not hang the browser forever.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Chrome for Testing's name for this platform, or `None` where it publishes no build.
pub fn platform() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "mac-arm64",
        ("macos", "x86_64") => "mac-x64",
        ("linux", "x86_64") => "linux64",
        ("linux", "aarch64") => "linux-arm64",
        ("windows", "x86_64") => "win64",
        ("windows", "x86") => "win32",
        _ => return None,
    })
}

/// Where the engine is installed under the browser's app-data folder.
pub fn engine_dir(root: &Path) -> PathBuf {
    root.join("engine")
}

/// The installed engine's program, if it is there.
pub fn installed(root: &Path) -> Option<PathBuf> {
    let platform = platform()?;
    let program = if cfg!(windows) {
        "chrome-headless-shell.exe"
    } else {
        "chrome-headless-shell"
    };
    let path = engine_dir(root)
        .join(format!("chrome-headless-shell-{platform}"))
        .join(program);
    path.is_file().then_some(path)
}

/// The stable engine's version and archive URL for `platform` from the versions list.
pub fn download_url(versions: &Value, platform: &str) -> anyhow::Result<(String, String)> {
    let stable = &versions["channels"]["Stable"];
    let version = stable["version"]
        .as_str()
        .context("the versions list names no stable version")?;
    let url = stable["downloads"]["chrome-headless-shell"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|download| download["platform"] == platform)
        .and_then(|download| download["url"].as_str())
        .with_context(|| format!("no browser engine is published for {platform}"))?;
    if !url.starts_with(DOWNLOAD_PREFIX) || url.contains("..") || !url.ends_with(".zip") {
        bail!("unexpected browser engine address {url}");
    }
    Ok((version.to_owned(), url.to_owned()))
}

/// Downloads and unpacks the engine into `root`, replacing an older one, and returns
/// its program. Callers run one install at a time.
pub async fn install(root: &Path) -> anyhow::Result<PathBuf> {
    let platform = platform().context("no browser engine is published for this platform")?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(DOWNLOAD_TIMEOUT)
        .build()?;
    let versions: Value = client
        .get(VERSIONS_URL)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let (_version, url) = download_url(&versions, platform)?;

    let staging = root.join("engine.partial");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let archive = staging.join("engine.zip");
    let mut response = client.get(&url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_DOWNLOAD)
    {
        bail!("the browser engine download is unexpectedly large");
    }
    let mut file = tokio::fs::File::create(&archive).await?;
    let mut received: u64 = 0;
    while let Some(chunk) = response.chunk().await? {
        received += chunk.len() as u64;
        if received > MAX_DOWNLOAD {
            bail!("the browser engine download is unexpectedly large");
        }
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    drop(file);

    let unpacked = staging.join("engine");
    let (from, into) = (archive.clone(), unpacked.clone());
    tokio::task::spawn_blocking(move || extract(&from, &into)).await??;
    let _ = std::fs::remove_file(&archive);
    let target = engine_dir(root);
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&unpacked, &target)?;
    let _ = std::fs::remove_dir_all(&staging);
    installed(root).context("the browser engine archive has no chrome-headless-shell program")
}

/// Unpacks `archive` into `into`, refusing entries that would land outside it.
fn extract(archive: &Path, into: &Path) -> anyhow::Result<()> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)?;
    let mut total: u64 = 0;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let Some(relative) = entry.enclosed_name() else {
            bail!("the browser engine archive has an unsafe entry");
        };
        let path = into.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&path)?;
            continue;
        }
        if entry.is_symlink() {
            bail!("the browser engine archive has a link entry");
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(&path)?;
        total += std::io::copy(&mut (&mut entry).take(MAX_UNPACKED - total), &mut file)?;
        if total >= MAX_UNPACKED {
            bail!("the browser engine archive unpacks unexpectedly large");
        }
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode & 0o755))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn versions(url: &str) -> Value {
        json!({"channels": {"Stable": {"version": "156.0.8078.47", "downloads": {
            "chrome-headless-shell": [{"platform": "mac-arm64", "url": url}]
        }}}})
    }

    #[test]
    fn picks_the_stable_engine_for_the_platform() {
        let url =
            format!("{DOWNLOAD_PREFIX}156.0.8078.47/mac-arm64/chrome-headless-shell-mac-arm64.zip");
        let (version, picked) = download_url(&versions(&url), "mac-arm64").unwrap();
        assert_eq!(version, "156.0.8078.47");
        assert_eq!(picked, url);
        assert!(download_url(&versions(&url), "win64").is_err());
    }

    #[test]
    fn refuses_downloads_from_anywhere_else() {
        for url in [
            "https://example.com/chrome-headless-shell-mac-arm64.zip",
            "http://storage.googleapis.com/chrome-for-testing-public/x.zip",
            "https://storage.googleapis.com/chrome-for-testing-public/../evil/x.zip",
            "https://storage.googleapis.com/chrome-for-testing-public/x.exe",
        ] {
            assert!(download_url(&versions(url), "mac-arm64").is_err(), "{url}");
        }
    }

    fn archive(entries: &[(&str, &[u8], u32)]) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut zip = zip::ZipWriter::new(file.reopen().unwrap());
        for (name, bytes, mode) in entries {
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(*mode);
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        file
    }

    #[test]
    fn unpacks_programs_runnable_and_refuses_escaping_entries() {
        let into = tempfile::tempdir().unwrap();
        let good = archive(&[
            ("shell/chrome-headless-shell", b"#!", 0o755),
            ("shell/data.pak", b"x", 0o644),
        ]);
        extract(good.path(), into.path()).unwrap();
        let program = into.path().join("shell/chrome-headless-shell");
        assert_eq!(std::fs::read(&program).unwrap(), b"#!");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&program).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111);
        }

        let other = tempfile::tempdir().unwrap();
        let evil = archive(&[("../escaped", b"x", 0o644)]);
        assert!(extract(evil.path(), other.path()).is_err());
        assert!(!other.path().parent().unwrap().join("escaped").exists());
    }

    #[test]
    fn finds_only_a_complete_install() {
        let root = tempfile::tempdir().unwrap();
        assert!(installed(root.path()).is_none());
        let Some(platform) = platform() else { return };
        let program = engine_dir(root.path())
            .join(format!("chrome-headless-shell-{platform}"))
            .join(if cfg!(windows) {
                "chrome-headless-shell.exe"
            } else {
                "chrome-headless-shell"
            });
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, b"").unwrap();
        assert_eq!(installed(root.path()), Some(program.clone()));

        // The hub prefers it over an installed browser, but a chosen browser still wins.
        let chosen = root.path().join("chosen");
        std::fs::write(&chosen, b"").unwrap();
        let hub = crate::browser::BrowserHub::new(crate::browser::BrowserConfig {
            root: root.path().to_path_buf(),
            executable: None,
            headless: true,
        });
        assert_eq!(hub.executable(), Some(program));
        assert!(hub.built_in());
        hub.set_executable(Some(chosen.clone()));
        assert_eq!(hub.executable(), Some(chosen));
    }
}
