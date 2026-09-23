# Updater notes (maintainers)

The desktop app ships `tauri-plugin-updater` (Rust crate `tauri-plugin-updater`
v2, registered in `src/lib.rs`). Until a release endpoint and signing key are
configured, `check_for_updates` returns `{state: "disabled"}` without touching
the network — see `src/updater.rs`. No private key material lives in this repo,
and none is needed to build, test, or run the app.

## One-time setup

Requires the Tauri CLI v2 (`npx -y @tauri-apps/cli@2 ...` from this directory).

1. Generate a signing keypair (do this once, on a trusted machine):

   ```sh
   npx -y @tauri-apps/cli@2 signer generate -w ~/.tauri/themis.key
   ```

   This writes the private key to `~/.tauri/themis.key` (protect it like a
   password; back it up offline) and prints the **public key**. Never commit
   the private key file.

2. Configure `tauri.conf.json`:

   ```json
   {
     "bundle": {
       "createUpdaterArtifacts": true
     },
     "plugins": {
       "updater": {
         "pubkey": "PASTE THE PRINTED PUBLIC KEY (not a file path)",
         "endpoints": [
           "https://github.com/<org>/<repo>/releases/latest/download/latest.json"
         ]
       }
     }
   }
   ```

   Endpoint URLs may use `{{current_version}}`, `{{target}}`
   (`linux|windows|darwin`) and `{{arch}}` placeholders. TLS (https) is
   enforced in production builds.

## Per-release flow

1. Bump versions (`Cargo.toml` files and `tauri.conf.json`).
2. Build with signing secrets available (CI recommended):

   ```sh
   export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/themis.key)"
   export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="<key password>"
   npx -y @tauri-apps/cli@2 build
   ```

   The bundler emits per-platform updater artifacts plus `.sig` files, e.g.
   on macOS `target/release/bundle/macos/Themis.app.tar.gz` and
   `Themis.app.tar.gz.sig`.

3. Publish a `latest.json` manifest at the configured endpoint. Static-file
   format (one entry per platform):

   ```json
   {
     "version": "0.2.0",
     "notes": "What changed.",
     "pub_date": "2026-09-23T12:00:00Z",
     "platforms": {
       "darwin-aarch64": {
         "signature": "CONTENTS OF Themis.app.tar.gz.sig",
         "url": "https://github.com/<org>/<repo>/releases/download/v0.2.0/Themis.app.tar.gz"
       },
       "darwin-x86_64": { "signature": "...", "url": "..." },
       "linux-x86_64": { "signature": "...", "url": "..." },
       "windows-x86_64": { "signature": "...", "url": "..." }
     }
   }
   ```

   Alternatively serve a dynamic endpoint: respond `204 No Content` when no
   update is available, or `200` with the JSON above when one is.

## How the app consumes it

- `check_for_updates` (frontend: `checkForUpdates()`) returns `UpdateStatus`:
  `disabled` (not configured), `up-to-date`, `available` (with `version` and
  `notes`), or `error` (with `message`). Failures are in-band; the command
  itself never rejects.
- This release only *checks*. Download/install is not wired to the UI yet;
  when it is, use the plugin's `Update::download`/`install` from a new
  command and require the `requireSignedVersion` plugin option consideration
  documented in `tauri-plugin-updater`'s `Config`.

References: <https://v2.tauri.app/plugin/updater/> and
<https://docs.rs/tauri-plugin-updater/latest/tauri_plugin_updater/>.
