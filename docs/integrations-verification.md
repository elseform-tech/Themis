# Integrations rework verification

Verified on 2026-10-06 on branch `codex/integrations-rework`. Installation acceptance uses disposable stores and synthetic files, with no real credentials. It does not install test packages into the user's active Themis configuration.

## Real public sources

Run the repeatable acceptance test:

```sh
cargo test -p themis-core --test plugin_imports real_public_plugin_and_skill_import_acceptance -- --ignored --nocapture
```

Result: one acceptance test passed. Each source is pinned:

| Source | Revision/version | Verified result |
| --- | --- | --- |
| [Superpowers](https://github.com/obra/superpowers) | `8ca22dba9a94f28898bbce59f2537ff4d87c747d` | 15 skills imported; 2 compatibility notices |
| [Anthropic skills](https://github.com/anthropics/skills) | `683bc88e56f3e09ba94f7055977f3d3aa499f202` | PDF skill and 12 text resources imported |
| [Official Claude plugin marketplace](https://github.com/anthropics/claude-plugins-official) | `d4226d062928f8d9505dbdeadd10217d23361052` | plugin-dev imported: 7 skills; 9 compatibility notices |
| Superpowers hook | Same pinned Superpowers revision | Reviewed upstream script executed through an explicit native RunStart adapter; returned parseable JSON |
| [Official filesystem MCP server](https://github.com/modelcontextprotocol/servers/tree/main/src/filesystem) | npm `@modelcontextprotocol/server-filesystem@2026.8.31` | Imported MCP JSON, initialized server, discovered 14 tools, read a synthetic marker file |

The upstream skill-creator instructions are 32,805 bytes, exceeding the existing 32,768-byte cap; the test confirms explicit rejection. Claude context injection from hook stdout and unsupported event/matcher semantics are not claimed to work. See [plugins.md](plugins.md) for compatibility and import limits, including unsupported archives, JSONC, and OAuth flows.

## Automated checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten --skip live_go_chat_completions_run
cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten --skip live_go_chat_completions_run
npm --prefix web run lint
npm --prefix web test
npm --prefix web run build
ruff check quality_dashboard
python3 -m unittest quality_dashboard.test_dashboard
```

Workspace Rust: 254 passed, 0 failed. Two filtered tests are the known host-specific synthetic Keychain write test and the live Go-provider test. The default suite ignores the separately executed public-source acceptance test and one existing documentation example. Web: 221 passed. Dashboard self-tests: 13 passed. Format, Clippy, ESLint, Ruff and builds pass; Vite retains its existing warning about chunks exceeding 500 kB.

Actual shared-server/CLI checks cover local skill/MCP imports, pasted bundle wrappers, rejected malformed imports preserving state, enable/disable, component removal preserving siblings, nested-hook CLI rejection, next-turn catalog refresh, old pinned references after added siblings, and calendar persistence across restart. Provider-request tests inspect the constructed system message: metadata and readable Markdown paths are present, full bodies are loaded lazily. Agent tools exercise denied mutations, successful persisted changes, current-chat automation creation, and partial updates preserving fields.

UI component tests exercise direct disable, View Markdown, More-menu-before-dialog behavior, uninstall confirmation, and marketplace/source association, and Ask Themis retaining the selected import source and options. These use mocked IPC; they are separate from the real CLI/backend checks. Live model relevance-based skill selection has not been verified. Hooks begin after provider/connection preparation; failures before RunStart do not emit RunEnd.

## Code health and native verification

The dashboard was refreshed through `/api/metrics` using rust-code-analysis 0.0.25. Mean callable metrics across 100 product files: cyclomatic 2.47, cognitive 2.19, maintainability 73.7. Inline Rust tests are included. Compared with `3c2b58a`, core plugin/import/hook maintainability improved; integrations UI maintainability fell from 85.6 to 83.5 as import and dialog branches were added. These averages do not establish correctness or a fixed maintainability threshold. Fresh Rust line coverage is 84.9% (12,860/15,151 lines); it does not measure browser/native interaction or live-model selection.

The macOS app bundle is rebuilt with:

```sh
cd crates/desktop
npx --yes @tauri-apps/cli build --bundles app --config '{"build":{"beforeBuildCommand":"","frontendDist":"../../web/dist"}}'
```

The rebuilt `target/release/bundle/macos/ThemisCode.app` embeds the current frontend asset (`index-CJu4bNAZ.js`). Its packaged CLI passed skill import, disable, and package uninstall against a disposable shared-server store.

Native visual and interaction verification remains unverified: computer-use controls reported that the Mac was locked, and it could not be automatically unlocked. The existing running app/server was not replaced during that blocked check. No UI screenshot or native acceptance claim is supplied. Skill/configuration changes take effect on the next run without an application restart; upgrading the running application binary still requires quitting the old UI, stopping its shared server, and launching the rebuilt app.
