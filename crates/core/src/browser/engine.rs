//! The built-in browser engine: Google's `chrome-headless-shell` (Chrome for Testing),
//! downloaded into app data the first time the browser is used. It is a plain program,
//! not an app bundle, so macOS never shows it in the Dock (a headless Google Chrome
//! appears there under recent apps), and the browser works without Chrome installed.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context};
use sha2::{Digest, Sha256};

use super::engine_pin::{ARCHIVES, VERSION};

/// Downloads are only taken from Google's Chrome for Testing bucket.
const DOWNLOAD_HOST: &str = "storage.googleapis.com";
const DOWNLOAD_PATH: &str = "/chrome-for-testing-public/";
/// The archives are about 100–120 MB; anything far larger is not the engine.
const MAX_DOWNLOAD: u64 = 400 * 1024 * 1024;
/// The unpacked engine is about 200–300 MB.
const MAX_UNPACKED: u64 = 3 * MAX_DOWNLOAD;
/// A slow connection gets time to finish; a stalled one does not hang the browser forever.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Chrome for Testing's name for this platform, or `None` where it publishes no build.
pub fn platform() -> Option<&'static str> {
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "mac-arm64",
        ("macos", "x86_64") => "mac-x64",
        ("linux", "x86_64") => "linux64",
        ("linux", "aarch64") => "linux-arm64",
        ("windows", "x86_64") => "win64",
        ("windows", "x86") => "win32",
        _ => return None,
    };
    expected_sha256(platform).map(|_| platform)
}

/// The pinned archive hash for `platform`.
fn expected_sha256(platform: &str) -> Option<&'static str> {
    ARCHIVES
        .iter()
        .find(|(name, _)| *name == platform)
        .map(|(_, sha256)| *sha256)
}

/// Where engines are installed under the browser's app-data folder.
pub fn engine_dir(root: &Path) -> PathBuf {
    root.join("engine")
}

/// The installed engine's program for the pinned version, if it is there.
pub fn installed(root: &Path) -> Option<PathBuf> {
    let platform = platform()?;
    let program = if cfg!(windows) {
        "chrome-headless-shell.exe"
    } else {
        "chrome-headless-shell"
    };
    let path = engine_dir(root)
        .join(VERSION)
        .join(format!("chrome-headless-shell-{platform}"))
        .join(program);
    path.is_file().then_some(path)
}

/// The pinned engine archive for `platform`.
pub fn archive_url(platform: &str) -> reqwest::Url {
    reqwest::Url::parse(&format!(
        "https://{DOWNLOAD_HOST}{DOWNLOAD_PATH}{VERSION}/{platform}/chrome-headless-shell-{platform}.zip"
    ))
    .expect("the pinned engine address is a valid URL")
}

/// Whether `url` is an https address in the Chrome for Testing bucket (checked on the
/// address the bytes actually came from, so a redirect cannot swap the source).
fn trusted(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some(DOWNLOAD_HOST)
        && url.port().is_none()
        && url.path().starts_with(DOWNLOAD_PATH)
}

/// Downloads the pinned engine, checks its SHA-256, unpacks it into `root` (replacing
/// older versions) and returns its program. Nothing is unpacked or run unless the hash
/// matches. Callers run one install at a time.
pub async fn install(root: &Path) -> anyhow::Result<PathBuf> {
    let platform = platform().context("no browser engine is published for this platform")?;
    let staging = root.join("engine.partial");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let installed = download_and_unpack(root, platform, &staging).await;
    let _ = std::fs::remove_dir_all(&staging);
    installed
}

async fn download_and_unpack(
    root: &Path,
    platform: &str,
    staging: &Path,
) -> anyhow::Result<PathBuf> {
    let expected = expected_sha256(platform).context("no pinned engine for this platform")?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(DOWNLOAD_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let url = archive_url(platform);
    let mut response = client.get(url).send().await?.error_for_status()?;
    if !response.status().is_success() || !trusted(response.url()) {
        bail!("the browser engine download did not come from Chrome for Testing");
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_DOWNLOAD)
    {
        bail!("the browser engine download is unexpectedly large");
    }
    let archive = staging.join("engine.zip");
    let mut file = tokio::fs::File::create(&archive).await?;
    let mut hash = Sha256::new();
    let mut received: u64 = 0;
    while let Some(chunk) = response.chunk().await? {
        received += chunk.len() as u64;
        if received > MAX_DOWNLOAD {
            bail!("the browser engine download is unexpectedly large");
        }
        hash.update(&chunk);
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    drop(file);
    let actual = format!("{:x}", hash.finalize());
    if actual != expected {
        bail!("the browser engine download does not match its pinned SHA-256");
    }

    let unpacked = staging.join(VERSION);
    let (from, into) = (archive, unpacked.clone());
    tokio::task::spawn_blocking(move || extract(&from, &into)).await??;
    let target = engine_dir(root);
    let _ = std::fs::remove_dir_all(&target);
    std::fs::create_dir_all(&target)?;
    std::fs::rename(&unpacked, target.join(VERSION))?;
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
    use std::io::Write;

    #[test]
    fn downloads_only_the_pinned_archive_from_chrome_for_testing() {
        let url = archive_url("mac-arm64");
        assert!(trusted(&url));
        assert!(url.as_str().ends_with(&format!(
            "{VERSION}/mac-arm64/chrome-headless-shell-mac-arm64.zip"
        )));
        for other in [
            "https://example.com/chrome-for-testing-public/x.zip",
            "http://storage.googleapis.com/chrome-for-testing-public/x.zip",
            "https://storage.googleapis.com:8443/chrome-for-testing-public/x.zip",
            "https://storage.googleapis.com/other-bucket/x.zip",
            "https://storage.googleapis.com.evil.test/chrome-for-testing-public/x.zip",
        ] {
            assert!(!trusted(&reqwest::Url::parse(other).unwrap()), "{other}");
        }
        // Every pinned hash is a full SHA-256.
        for (_, sha256) in ARCHIVES {
            assert!(sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit()));
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
            .join(VERSION)
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
