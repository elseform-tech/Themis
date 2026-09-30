#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
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
