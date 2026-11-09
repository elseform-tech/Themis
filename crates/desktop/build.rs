fn main() {
    // Windows ships the console `themis` CLI next to the app (`bundle.externalBin` in
    // tauri.windows.conf.json), staged by scripts/stage-cli.mjs before a bundle build. tauri-build
    // needs the file to exist, and ordinary cargo builds and tests do not stage it, so they get an
    // empty stand-in, which the app never uses (see `server::agent_path`).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let triple = std::env::var("TARGET").expect("cargo sets TARGET");
        let staged = std::path::Path::new("binaries").join(format!("themis-cli-{triple}.exe"));
        if !staged.exists() {
            std::fs::create_dir_all("binaries").expect("create binaries/");
            std::fs::write(&staged, b"").expect("write the CLI stand-in");
        }
    }
    tauri_build::build();
}
