//! Console entry point for the shared application CLI.
fn main() {
    themis_desktop::install_runtime();
    if let Err(error) = tauri::async_runtime::block_on(themis_desktop::cli::run()) {
        eprintln!("themis: {error}");
        std::process::exit(1);
    }
}
