//! Console entry point for the shared application CLI.
#[tokio::main]
async fn main() {
    if let Err(error) = themis_desktop::cli::run().await {
        eprintln!("themis: {error}");
        std::process::exit(1);
    }
}
