#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
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
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(std::ffi::OsStr::new("--themis-server")) {
        let Some(data_dir) = args.next() else {
            eprintln!("themis: missing server data directory");
            std::process::exit(2);
        };
        if let Err(error) = themis_desktop::run_headless_server(data_dir.into()) {
            eprintln!("themis: {error}");
            std::process::exit(1);
        }
    } else {
        themis_desktop::run();
    }
}
