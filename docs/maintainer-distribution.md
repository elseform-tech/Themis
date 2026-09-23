# Themis distribution guide (maintainers)

How versions, builds, signing, updates, and CI fit together in this repo.

## Versioning

The version is `0.1.0` in four places — keep them in sync on every release:

- `crates/core/Cargo.toml` (`themis-core`)
- `crates/desktop/Cargo.toml` (`themis-desktop`)
- `crates/desktop/tauri.conf.json` (`version`)
- `web/package.json` (`version`)

Derived identifiers: the Tauri app id is `ai.themis.desktop` (also the OS
keychain service name), the product name is `Themis`, and the Go provider's
user agent is `themis/<themis-core version>`.

## Builds

Prerequisites: a Rust toolchain (see `rust-version = "1.91"` in the crate
manifests) and Node for `web/`.

```sh
npm --prefix web run build     # typecheck (tsc) + vite bundle into web/dist
npx tauri build                # from crates/desktop; bundles web/dist
npx tauri dev                  # from crates/desktop; dev server on :1420
```

`crates/desktop/tauri.conf.json` wires the frontend: `frontendDist` is
`../../web/dist`, the dev URL is `http://localhost:1420`, and the
before-dev/before-build commands run the matching npm scripts, so plain
`npx tauri dev|build` from `crates/desktop` is sufficient.

### Per OS

Only macOS is configured today: `bundle.targets` is `["app", "dmg"]` and
`bundle.active` is `true`. Bundles land under
`crates/desktop/target/release/bundle/`.

- **macOS**: `.app` + `.dmg` out of the box.
- **Windows / Linux**: not configured. To add them, extend `bundle.targets`
  (e.g. `nsis`, `msi`, `appimage`, `deb`) and build on a runner for each OS —
  Tauri does not cross-compile bundles. The Rust code has no OS-specific
  branches, but the worktree/git paths and keychain backend (`keyring`
  crate) should be smoke-tested per platform.

## Apple signing and notarization

Signing-ready but unsigned: `tauri.conf.json` already sets the `DeveloperTool`
category and `bundle.macOS.minimumSystemVersion`, but there is no signing
identity and no entitlements file yet. Unsigned builds run locally but
Gatekeeper will block them on other machines. To ship:

1. Enroll in the Apple Developer Program and create a **Developer ID
   Application** certificate, e.g. identity
   `Developer ID Application: Your Name (TEAMID123)`.
2. Export the Tauri signing variables in the build environment:
   `APPLE_CERTIFICATE` (base64 `.p12`), `APPLE_CERTIFICATE_PASSWORD`,
   `APPLE_SIGNING_IDENTITY` (the identity above), plus `APPLE_ID`,
   `APPLE_PASSWORD` (app-specific password), and `APPLE_TEAM_ID` for
   notarization.
3. If the app needs capabilities beyond the default sandbox, add an
   entitlements file under `crates/desktop/` and point
   `bundle.macOS.entitlements` at it. No entitlements file ships in this repo;
   unsigned local builds do not need one.
4. `npx tauri build --target universal-apple-darwin` produces a signed,
   notarized `.dmg` when the variables are present.

## Updater endpoint and key setup

Status: **not wired**. The frontend contract (`UpdateStatus`, `state:
"disabled" | "up-to-date" | "available" | "error"`, `check_for_updates`
command in `web/src/lib/tauri.ts`) exists and the Settings screen renders all
four states, but `tauri.conf.json` has no `updater` section and
`tauri-plugin-updater` is not a dependency — builds report `disabled`.

To enable in-app updates:

1. Add `tauri-plugin-updater` to `crates/desktop` (dependencies +
   `tauri-build` features) and register it in `lib.rs`.
2. Generate a keypair: `npx tauri signer generate -w ~/.tauri/themis.key`.
   The **public** key goes into `tauri.conf.json` under
   `plugins.updater.pubkey`; the **private** key stays out of the repo (CI
   secret, e.g. `TAURI_SIGNER_PRIVATE_KEY` + password secret).
3. Host a versioned update manifest (e.g. `latest.json`) per platform on
   stable HTTPS URLs and set `plugins.updater.endpoints` to them. The
   `check_for_updates` backend should query that manifest and map the result
   onto the `UpdateStatus` contract.
4. Sign each release's bundles (`npx tauri signer sign`) and publish the
   signatures alongside the manifest.

## CI overview

Status: **no workflows ship** — this repo has no `.github/` directory
(verified). Any workflow added should run at least these gates, which mirror
the repo's documented commands:

```sh
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
npm --prefix web test
npm --prefix web run build
```

Suggested layout when CI is introduced: a lint job (fmt + clippy), a test job
(cargo tests + web tests + web build), and — once signing secrets exist — a
per-OS release job running `npx tauri build` on tag push.
