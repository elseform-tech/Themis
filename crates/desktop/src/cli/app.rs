//! Installing the CLI and opening or closing the desktop app.

use super::*;

pub(super) fn install_cli(directory: &std::path::Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        if !directory.is_absolute() {
            return Err("CLI installation directory must be absolute".to_owned());
        }
        let executable = std::env::current_exe()
            .and_then(|path| path.canonicalize())
            .map_err(|error| error.to_string())?;
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        let destination = directory.join("themis");
        match std::fs::symlink_metadata(&destination) {
            Ok(_) if std::fs::read_link(&destination).ok().as_ref() == Some(&executable) => {}
            Ok(_) => {
                return Err(format!(
                    "{} already exists; choose another directory",
                    destination.display()
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::os::unix::fs::symlink(&executable, &destination)
                    .map_err(|error| error.to_string())?
            }
            Err(error) => return Err(error.to_string()),
        }
        println!(
            "Installed {}. Add {} to PATH if needed; shell configuration was not changed.",
            destination.display(),
            directory.display()
        );
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
        Err(
            "Build the Windows console CLI with cargo install --path crates/desktop --bin themis"
                .to_owned(),
        )
    }
}

#[cfg(target_os = "macos")]
pub(super) fn local_app_bundle() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
        .and_then(|dir| {
            [
                dir.parent()?.parent()?.to_path_buf(),
                dir.parent()?.join("release/bundle/macos/Themis.app"),
                dir.join("bundle/macos/Themis.app"),
            ]
            .into_iter()
            .find(|path| {
                path.is_dir() && path.extension().is_some_and(|extension| extension == "app")
            })
        })
}

pub(super) fn launch_app() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let status = {
        let mut command = std::process::Command::new("open");
        if let Some(bundle) = local_app_bundle() {
            command.arg("-n").arg(bundle);
        } else {
            command.args(["-a", "Themis"]);
        }
        command.status()
    };
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("cmd")
        .args(["/C", "start", "", "Themis.exe"])
        .status();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let status = std::process::Command::new("gtk-launch")
        .arg("ai.themis.desktop")
        .status();
    app_command_result(status, "launch")
}

pub(super) fn quit_app() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "tell application (item 1 of argv) to quit",
            "-e",
            "end run",
        ])
        .arg(local_app_bundle().map_or_else(
            || "Themis".to_owned(),
            |path| path.to_string_lossy().into_owned(),
        ))
        .status();
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-Process -Name 'Themis','themis-desktop' -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | ForEach-Object { $_.CloseMainWindow() | Out-Null }",
        ])
        .status();
    #[cfg(target_os = "linux")]
    return quit_linux_app();
    #[cfg(not(target_os = "linux"))]
    app_command_result(status, "quit")
}

#[cfg(target_os = "linux")]
pub(super) fn quit_linux_app() -> Result<(), String> {
    for entry in std::fs::read_dir("/proc").map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let name = std::fs::read_link(path.join("exe"))
            .ok()
            .and_then(|exe| exe.file_name().map(std::ffi::OsStr::to_os_string));
        if name.as_deref() != Some(std::ffi::OsStr::new("themis-desktop")) {
            continue;
        }
        let args = std::fs::read(path.join("cmdline")).unwrap_or_default();
        if args
            .split(|byte| *byte == 0)
            .any(|arg| arg == b"--themis-server")
        {
            continue;
        }
        let status = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status();
        app_command_result(status, "quit")?;
    }
    Ok(())
}

pub(super) fn app_command_result(
    status: std::io::Result<std::process::ExitStatus>,
    action: &str,
) -> Result<(), String> {
    match status {
        Ok(code) if code.success() => Ok(()),
        Ok(code) => Err(format!("could not {action} Themis: {code}")),
        Err(error) => Err(format!("could not {action} Themis: {error}")),
    }
}
