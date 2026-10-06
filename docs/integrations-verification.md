# Integrations rework verification

Verified on 2026-10-06 on branch `codex/integrations-rework`. Installation acceptance uses disposable stores and synthetic files, with no real credentials. The automated acceptance tests do not install packages into the user's active configuration. The later native UI check created explicitly disposable project-scoped fixtures and removed them after verification.

## Real public sources

Run the repeatable acceptance test:

```sh
cargo test -p themis-core --test plugin_imports real_public_plugin_and_skill_import_acceptance -- --ignored --nocapture
```

Result: one acceptance test passed. Each source is pinned:

| Source | Revision/version | Verified result |
| --- | --- | --- |
| [Superpowers](https://github.com/obra/superpowers) | `8ca22dba9a94f28898bbce59f2537ff4d87c747d` | 15 skills imported; 2 compatibility notices |
| [Anthropic skills](https://github.com/anthropics/skills) | `683bc88e56f3e09ba94f7055977f3d3aa499f202` | PDF, DOCX and XLSX standalone skills imported (12/61/53 resources) |
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

Workspace Rust: 266 passed, 0 failed. Two filtered tests are the known host-specific synthetic Keychain write test and the live Go-provider test. The default suite ignores the separately executed public-source acceptance test and one existing documentation example. Web: 275 passed across 26 files (20 integration-browser cases). Dashboard self-tests: 13 passed. Format, Clippy, ESLint, Ruff and builds pass; Vite retains its existing warning about chunks exceeding 500 kB.

Actual shared-server/CLI checks cover local skill/MCP imports, pasted bundle wrappers, rejected malformed imports preserving state, enable/disable, component removal preserving siblings, nested-hook CLI rejection, next-turn catalog refresh, old pinned references after added siblings, and calendar persistence across restart. Provider-request tests inspect the constructed system message: metadata and readable Markdown paths are present, full bodies are loaded lazily. Agent tools exercise denied mutations, successful persisted changes, current-chat automation creation, and partial updates preserving fields.

UI component tests exercise accessible enable/disable switches, full pretty Markdown, uninstall confirmation/cancellation, sibling preservation, public install/uninstall grouping, disabled-parent gating, discovery controls and marketplace/source association. They assert the absence of configuration and agent-help controls. These use mocked IPC; they are separate from the real CLI/backend checks. Live model relevance-based skill selection has not been verified. Hooks begin after provider/connection preparation; failures before RunStart do not emit RunEnd.

## Code health and native verification

The dashboard was refreshed through `/api/metrics` using rust-code-analysis 0.0.25. Mean callable metrics across 101 product files: cyclomatic 2.50, cognitive 2.23, maintainability 73.5. Inline Rust tests are included. Compared with `3c2b58a`, core plugin/import/hook maintainability improved; integrations UI maintainability fell from 85.6 to 81.2; the current 202-line module has mean cyclomatic 2.58 and cognitive 2.82. Lifecycle and preview branches remain covered by focused tests. These averages do not establish correctness or a fixed maintainability threshold. Fresh Rust line coverage is 85.5% (13,307/15,568 lines); it does not measure browser/native interaction or live-model selection.

The macOS app bundle is rebuilt with:

```sh
cd crates/desktop
npx --yes @tauri-apps/cli build --bundles app --config '{"build":{"beforeBuildCommand":"","frontendDist":"../../web/dist"}}'
```

The rebuilt `target/release/bundle/macos/ThemisCode.app` embeds the current frontend asset (`index-CSLvuBGd.js`). Its packaged CLI passed skill import, disable, and package uninstall against a disposable shared-server store.

Initial native verification was blocked: computer-use controls reported that the Mac was locked, and it could not be automatically unlocked. The existing running app/server was not replaced during that blocked check. At that stage native acceptance remained unverified. Skill/configuration changes take effect on the next run without an application restart; upgrading the running application binary still requires quitting the old UI, stopping its shared server, and launching the rebuilt app.

## Native UI follow-up

After the Mac became available, the rebuilt native window and refreshed shared backend were exercised through computer-use controls. Plugin JSON import, direct disable/re-enable, bundled skill Markdown viewing, More action menus and Configure dialogs passed. Skills from common user directories were visible. Native MCP JSON import defaulted to disabled; enable and explicit Test connected the official filesystem MCP server and showed its tools. Native hook import defaulted to disabled; an invalid plain-text response showed an error, then a saved JSON-producing command returned the expected parsed marker.

Automation creation exposed only Instructions, Repeat and Time by default. Advanced contained destination, optional name, time zone and enabled state. A paused disposable daily automation was saved, reopened with the same instructions and 09:00 time, then deleted. The plugin, MCP and hook fixtures were removed through the shared CLI. No provider task was sent or scheduled to run.

Historical configuration screenshots: [MCP connection](verification/native-mcp.png), [hook result](verification/native-hook.png). The [automation form](verification/native-automation.png) has been refreshed to the final minimal design. The native pass found unreadable discovery IDs in row labels; the implementation now preserves internal IDs but displays parsed skill names.

## Cancellation follow-up

A user-started native chat remained in Stopping during a stalled provider/compaction wait. The root cause was an awaited model request without cancellation or a compaction deadline. Provider and summary waits now observe the shared stop flag; compaction has a 120-second deadline and preserves the original context on timeout. Active tools retain their safe completion boundary.

Regression tests cancel stalled answer and summary requests within 500 ms. A real shared-server CLI test stops stalled compaction and observes `running=false` within one second. Double Escape within 500 ms calls the current Stop action once, including with an unfocused or disabled composer; single Escape still dismisses suggestions. Mock UI tests cover these keyboard paths. Active native double-Escape cancellation and live-provider compaction success have not been exercised with real credentials.

The final native follow-up verified installed/public search, scrolling to the bottom with fixed navigation, and public plugin-dev skills and Markdown before installation. Ask Themis produced an editable unsent draft with the search and project context; `/manage` followed by Tab inserted the management skill chip without sending. The classification follow-up moves discovered/imported standalone skills out of Plugins and `@`, keeping genuine one-skill plugin packages visible. CLI assertions cover imported skill/MCP types and marketplace previews; legacy import classification and invalid package types are regression-tested.

Final native screenshots: [scrolling list](verification/native-integrations-scroll.png), [public plugin skill list](verification/native-public-preview.png). The rebuilt app recovered the previously interrupted chat to idle and preserved its history.

Plugins and Skills now separate Installed from Not installed. Disabled packages remain installed. The standalone catalog initially includes three verified pinned Anthropic skills; it does not enumerate every public marketplace skill. Uninstalled standalone skill details preview the actual pinned repository `SKILL.md` through the read-only `preview_repository` action; the registry remains unchanged. Installed skill/plugin details render the complete captured instruction body; original frontmatter stays preserved in storage. Plugin details show the actual skill count and all skill names, including an honest empty state for packages without skills. Rows use source-supplied icons, with document/type glyphs when artwork is unavailable.

## Final edge-case pass

Independent audits found and fixed agent-created skill classification, disabled standalone re-enable, and failed-image fallback. Regression tests preserve genuine disabled-plugin child gating, editable import-error dialogs, and existing drafts when no chat is selected. Native visual inspection found Tailwind Preflight made icon images/SVGs block elements; a test now loads the actual reset with the reference styles, and inline references explicitly keep their icon and label together. Mixed-reference deletion/undo and caret preservation during metadata refresh are tested.

A scripted mock model exercises marketplace preview, installation, disabling, component removal and package uninstall through the real CLI, five approval decisions and the shared management API. Follow-up provider requests prove the catalog refreshes after installation/removal while skill bodies stay lazy. These are real orchestration/transport paths with a mock model, not evidence of live-model relevance selection.

The final user correction removes Ask Themis controls and their draft helpers from Integrations and Automations. Earlier Ask screenshots/checks describe an intermediate version. Management skills remain invokable in normal chat.


## Complete source viewer

Core repository preview tests compare original file bytes and registry state; the shared CLI test additionally checks a pinned local Git revision, subdirectory and supporting resources, then installs the same source. Frontend regressions cover long source tails, extra frontmatter, source paths whose folder differs from skill metadata, multiple skill names/counts, rendered headings, zero-skill packages and previews that never call import. Imported source paths are stored explicitly; older root/nested packages retain compatible lookup. The final viewer uses the existing Markdown renderer only. Frontmatter remains in captured source but is omitted from the pretty view. Plugin View initially shows its skill count and simple skill rows, each with one view icon; choosing a row renders the complete body, with no Source/Rendered controls.

Shared save synchronization updates the captured Markdown when parsed skill fields change and the captured source was not edited explicitly. Instructions-only changes preserve the original header; metadata edits preserve unrelated frontmatter. Explicit file edits take precedence. Original unedited source bytes remain unchanged.


## Minimal control follow-up

Removed Ask Themis controls, dialog intent fields, export actions, separate test navigation and duplicate JSON-file import pickers. This intermediate reduction retained More/Configure, which the final browser removes entirely. The final rows retain View, a labelled switch, and confirmed Uninstall; public rows retain View and Install. MCP/hook configuration and tests run through chat or CLI. The standalone catalog still supports one-click install.

The final native viewer check uses a disposable two-skill package with a 2,559-character Markdown file, verifies the second skill independently, and reaches the final source marker. Native command selection inserts plugin and skill references on one baseline without sending. Canonical management-ID search is regression-tested after native testing found the old name-only matcher.

Composer keyboard navigation now scrolls the selected suggestion into view with nearest alignment for both skill and plugin lists. Regression tests walk long lists downward, upward and through both wrap boundaries, verifying the scroll target equals the selected option. Existing bounded popup scrolling is reused.


## Final integration browser and lifecycle controls

The latest user corrections supersede earlier configuration checks. Integrations browses and previews packages, skills, MCP and hooks, with basic Install, clear enable/disable switches and confirmed Uninstall controls. Creation, arbitrary-source imports, configuration, tests and advanced management run through chat management skills/shared APIs or the CLI. There are no Configure, Add, More or Ask Themis controls in this browser. Earlier native import and configuration screenshots document intermediate implementation checks. Automations remains separate. Row hover uses a subtle background; keyboard selections keep a clear indicator.


The final native pass verifies a switch turning off/on with both its visible label and accessible checked state, discovered skill switching, confirmed PDF removal returning it to Not installed, public Word Markdown preview, seven-skill plugin-dev preview, complete Markdown reaching its final marker, and scrolling to the last skills row while search/navigation remain fixed. `/manage-sk` selects Manage skills with Tab without sending. Downward/upward navigation and wraparound keep the selected suggestion visible. Automations exposes Instructions, Repeat and Time with Advanced collapsed. No provider message was sent.

Evidence: [lifecycle switches](verification/native-lifecycle.png), [bundled skills](verification/native-bundled-skills.png), [full Markdown tail](verification/native-full-markdown.png), [public standalone preview](verification/native-skills-available.png), [inline references](verification/native-inline.png), [composer scrolling](verification/native-composer-scroll.png). Disposable plugin, PDF wrapper, discovery file and empty native test chat were removed after verification. The user-installed browser-use package and original chat were preserved.

Composer descriptions are limited to two visible lines while their full text remains available in the tooltip. Discovered skill origins show User/Project · Discovered skills instead of internal hashes; canonical identifiers still match and persist. The final PromptInput module is 129 lines, mean cyclomatic 3.85, cognitive 5.88 and maintainability 74.8 (down from 75.1 before this display correction). No frontend line-coverage report is available.

Final native build confirmed the two-line composer descriptions and readable discovery origins, downward navigation to the last offered skill, both wrap directions and `@browser` selection with Tab without sending. The final disposable chat was empty and removed; reopening the app refreshed its sidebar. MCP details expose saved transport/command/status only, with no editor or Test action. [Final clean browser](verification/native-final-ui.png).
