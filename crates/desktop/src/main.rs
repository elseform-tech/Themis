#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    themis_desktop::install_runtime();
    let cli_mode = std::env::args_os().next().is_some_and(|path| {
        std::path::Path::new(&path).file_name() == Some(std::ffi::OsStr::new("themis"))
    }) || std::env::args_os().nth(1).as_deref()
        == Some(std::ffi::OsStr::new("--themis-cli"));
    if cli_mode {
        if let Err(error) = tauri::async_runtime::block_on(themis_desktop::cli::run()) {
            eprintln!("themis: {error}");
            std::process::exit(1);
        }
        return;
    }
    // Opened from Finder or the Dock: find the programs MCP servers and hooks use.
    themis_desktop::adopt_login_shell_path();
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(std::ffi::OsStr::new("--themis-server")) {
        let Some(data_dir) = args.next() else {
            eprintln!("themis: missing server data directory");
            std::process::exit(2);
        };
        let with_app = args.next().as_deref() == Some(std::ffi::OsStr::new("--with-app"));
        if let Err(error) = themis_desktop::run_headless_server(data_dir.into(), with_app) {
            eprintln!("themis: {error}");
            std::process::exit(1);
        }
    } else {
        themis_desktop::run();
    }
}
