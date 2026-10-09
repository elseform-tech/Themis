# Integrations audit — 9 October 2026

Scope: native macOS app, shared server/CLI, current source, and isolated integration tests. The original seven-point request was an audit. Subsequent requests explicitly authorized increasing the skill instruction limit, removing Skills Discover, and adding Recent to the sidebar. The later authorized follow-up is delivered below; the original findings are retained as historical observations.

## Journey and findings

1. **Choose Installed or Discover — needs improvement.** The controls expose the correct pressed state to accessibility, but their toolbar lacks the selected styling used by the category tabs. Add a persistent accent/underline with a brief shimmer; motion should supplement a readable selection. See `web/src/screens/Plugins.css` and `Plugins.tsx`.
2. **Wait for a marketplace — needs improvement.** Native checking showed only a small inline spinner and completed/total text, leaving most of the panel empty. Use the companion in the content area's center with a surrounding progress ring and actual completed/total percentage. Before the total is known, use indeterminate progress instead of a fabricated percentage. Keep navigation usable.
3. **Load a marketplace — high priority.** `run_marketplace_scan` awaits each package sequentially. The refreshed official marketplace had 315 entries; one observation was only 52 complete after approximately 138 seconds. This is a sampled live observation, not a controlled benchmark or total-duration measurement. Use a bounded worker pool, reuse the persisted cache, and consider showing checked entries as they arrive. Preserve scan revision checks and installation of the exact inspected contents.
4. **Understand compatibility — needs improvement.** Badges carry color, but reasons use muted, clipped text; some unsupported capabilities collapse to a generic explanation. Show an amber/red warning icon with the specific reason and affected field, wrapping or expanding accessibly. Keep “unsupported,” “partially supported,” and “check failed” distinct.
5. **Find skills — simplified as requested.** Skills had a separate, hardcoded three-item Anthropic catalog, using a different frontend inspection/cache flow from Plugins. That Discover view and its unused code are removed. Skills still lists installed, bundled, and locally discovered skills. Standalone chat/CLI imports remain supported.
6. **Relaunch and retain installations — tested, with a visibility defect identified.** A PDF skill installed through the native UI persisted with the same content, revision and enabled state after window relaunch. A copy of that registry retained both PDF and frontend-design across two fresh server processes using the rebuilt executable. The temporary PDF installation was removed afterward, restoring the original listing. The original frontend-design installation also remained byte-for-byte equivalent in the plugin listing after the main backend was rebuilt and restarted.

   A separate reproducible issue made skills appear missing: a rejected discovered skill contains no loadable skill rows, and the UI hides that wrapper. The 86,932-byte local design-taste-frontend skill was rejected by the former 32 KiB limit. With the new 256 KiB body limit it is fully loaded and visibly available in the native app. This explains one concrete disappearance mechanism; it does not establish the cause of every reported missing installation. Rejected skills can still be hidden for other validation errors. Different QA app-data profiles also have different registries; switching profiles can look like data loss.
7. **Filter and search — needs improvement.** Search and Personal/Public already share state across Installed/Discover and across categories. Status filters differ appropriately between installed activation and catalog compatibility. Use a compact shared search/source toolbar, visible result count and reset, with view-specific status options. Avoid misleading “nothing installed” text when a filter hides installed rows. The current Available filter also includes Check failed entries, which deserves clearer treatment.

## Changes delivered

- Skill instruction bodies accept up to 256 KiB of UTF-8 text without truncation. Import, local discovery, creation verification and runtime validation agree. Frontmatter is excluded from this body limit. Existing path, resource and script safeguards remain. Older marketplace compatibility snapshots are invalidated so old size-limit failures are rechecked.
- Skills has one installed/local library view. Plugins retains Discover.
- Recent shows eight chats with the latest saved conversation activity across open projects, with selection and status. Projects appears first, followed by Recent with a clock heading icon, aligned chat icons, and single-line ellipsized titles. Project-name captions are removed; full title/project context remains in hover text. The sidebar has a draggable and keyboard-accessible width control (240–560 px), persisted as a local UI preference. Empty chats remain in their project until they have activity. Opening a chat alone does not reorder it. Ordering uses the existing transcript sequence and requires no additional writable store.

## Verification evidence

Native screenshots and raw local logs are in `/tmp/themis-integration-audit-20261009/`:

| Evidence | File |
| --- | --- |
| Original Installed view | `01-installed.png` |
| Original marketplace compatibility cards | `02-marketplace.png` |
| Original marketplace loading | `05-loading.png` |
| Installed PDF after relaunch | `06-pdf-after-restart.png` |
| Empty MCP and Hooks views | `07-mcp.png`, `08-hooks.png` |
| Skills without Discover | `09-skills-without-discover.png` |
| Previously hidden skill recovered | `10-recovered-large-skill.png` |
| Final Projects-first, resizable sidebar | `12-projects-recent-resizable.png` |

The public acceptance command imports pinned Superpowers, Anthropic skills and the official plugin catalog in isolated storage. It imported the formerly rejected 32,805-byte skill-creator without truncation, imported PDF/Word/spreadsheet resources, executed the reviewed Superpowers hook through an explicit native RunStart adapter, initialized `@modelcontextprotocol/server-filesystem@2026.8.31`, discovered 14 tools, and read a synthetic marker. It does not establish authenticated service health or Claude-specific hook context-injection semantics.

```sh
cargo test -p themis-core --test plugin_imports real_public_plugin_and_skill_import_acceptance -- --ignored --nocapture
cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build -p themis-desktop --bins
npm --prefix web run lint
npm --prefix web test
npm --prefix web run build
ruff check quality_dashboard crates/core/builtins/create-skill/scripts/verify.py
python3 -m unittest quality_dashboard.test_dashboard
```

The unrestricted Rust run failed at the macOS synthetic Keychain-write test: the host rejected its synthetic credential with “The user name or passphrase you entered is not correct.” The explicit exclusion above does not claim Keychain persistence passed. Live Go-provider tests remain opt-in; no real credential was used for these checks.

## Code health

Dashboard metrics were refreshed and its 14 self-tests passed. After removing the duplicate catalog, Plugins.tsx fell from 367 to 297 LOC; mean cyclomatic complexity fell from 2.98 to 2.89, cognitive complexity from 3.24 to 2.91, and maintainability rose from 80.8 to 81.3. Discovery complexity also decreased. The overall mean maintainability changed from 73.4 to 73.3 as the mix of functions changed; no score-driven refactor was added. The Python verifier retains its pre-existing high cognitive complexity of 49.

The available Rust coverage artifact reports 88.2% (18,575/21,060 lines), generated before this task. It is historical coverage, not coverage of these changes. Frontend coverage is unavailable. Final product averages are recorded below.

## Final evidence and results

Repository copies of the key screenshots: [original selection](images/integrations-audit-20261009/01-installed.png), [marketplace loading](images/integrations-audit-20261009/05-loading.png), [recovered skill](images/integrations-audit-20261009/10-recovered-large-skill.png), [final sidebar](images/integrations-audit-20261009/12-projects-recent-resizable.png).

- Rust workspace: 340 passed, 0 failed, 3 ignored, 1 explicitly filtered Keychain test. The ignored public acceptance case was run separately and passed; the other ignored cases are the live-provider test and a documentation example.
- Web suite: 301 tests passed across 29 files.
- Native verification: Skills has no Discover; the large local skill is visible; Recent has its clock icon, opens the correct project chat, and hides/returns with the sidebar. Projects appears above Recent, and titles are single-line with ellipses and no project-name captions.
- Sidebar test: cross-project ordering, completion updates, correct selection, collapsed visibility, pointer movement/release, keyboard bounds, and remount persistence passed. Native drag automation itself returned `noWindowsAvailable`; physical pointer dragging is not claimed as verified.
- Formatting, Clippy, ESLint, Ruff and production web build passed. The build retains its existing large-chunk warning. Dashboard self-tests: 14 passed.
- During development, an appearance test exceeded its 5-second timeout while Rust compilation was running; a fresh full web run subsequently passed without a code change for that timeout. Intermediate Rust runs included deliberate failing Recent assertions while the backend field was not yet implemented; final totals above supersede those runs.

Final dashboard averages across 106 product files: cyclomatic 2.56, cognitive 2.25, maintainability 73.4. `web/src/screens/Sidebar.tsx`: 176 LOC, cyclomatic 2.41, cognitive 2.49, maintainability 82.1. `web/src/state/store.tsx`: 316 LOC, cyclomatic 2.35, cognitive 3.79, maintainability 74.0. The event subscription gained a small amount of complexity to refresh activity ordering and reject stale responses; this is retained to preserve correctness.

## Authorized follow-up delivery

Installed and Discover now have a persistent selected treatment with shimmer, independent search/source/status filters, result counts and reset. Marketplace checking uses four concurrent workers and shows the companion, progress ring and actual completed percentage. Compatibility and connection warnings are collapsed by default and expose the full reason when expanded. Rejected local skills remain visible as unavailable. Integration notifications use the existing six-second side toast.

MCP rows show the complete quoted command and arguments, with status, enable switch and delete aligned. Enabled MCP connections start independently in the background on project entry, with a 30-second startup bound; failure does not block the app or other servers. Runs reuse the shared transport while retaining per-run tool approvals. Disabled imports remain disabled. MCP details lists cached tool names and descriptions after connection.

Native macOS proof: a fresh app entry connected Playwright without opening a chat or pressing Refresh. Opening its details showed **25 tools**, including browser_close and browser_navigate with descriptions. A failing browser-use connection did not prevent Playwright or navigation from working. The latest backend was rebuilt and restarted; this is a development app wrapper using current frontend assets, not a signed release package. Sidebar pointer resizing, keyboard bounds and width retention after relaunch were also verified.

The real 315-entry marketplace observation completed with 227 reports and 88 check failures (205 available entries). This was a resumed/cache-assisted observation, not a controlled speed benchmark. The isolated concurrency regression proves overlapping inspections bounded at four. Native checks verified separate searches, expandable warnings, the full MCP command and timed notification expiry.

Screenshots: [loading](images/integrations-followup-20261009/loading.png), [collapsed compatibility](images/integrations-followup-20261009/warnings-final.png), [MCP connection](images/integrations-followup-20261009/mcp-connected.png), [MCP tools](images/integrations-followup-20261009/mcp-tools.png), [timed toast](images/integrations-followup-20261009/toast.png).

### Follow-up verification

- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`, `cargo build -p themis-desktop --bins`: passed.
- `cargo test -p themis-desktop --test plugins_e2e mcp_startup_is_background_isolated_and_reuses_connections`: passed. Covers healthy/failed/disabled servers, background return, shared CLI metadata, reuse, policy denial and persisted configuration after restart, using isolated mock HTTP.
- `npm --prefix web run lint`, `npm --prefix web test`, `npm --prefix web run build`: passed; 306 tests across 29 files. Existing Vite large-chunk warning remains.
- `python3 -m unittest quality_dashboard.test_dashboard`: 14 passed; `ruff check quality_dashboard`: passed.
- The dashboard was refreshed after the changes. AST analysis covers 107 product files: average cyclomatic 2.56, cognitive 2.24, maintainability 73.5. Inline Rust tests remain in source metrics. Touched modules (cyclomatic/cognitive/maintainability): connections.rs 3.65/3.54/65.5; state/mcp.rs 1.94/0.55/78.5; store.tsx 2.44/3.75/74.0; Plugins.tsx 3.06/3.26/81.3. Plugins increased from 2.89/2.91/81.3 before the follow-up because of connection/filter branches; no score-driven refactoring was introduced.
- Available Rust coverage is historical (8 October): 88.2%, 18,575/21,060 lines; it is not coverage of this patch. Fresh frontend coverage is unavailable.

Real native MCP initialization/tool metadata is verified; authenticated remote services, live model-provider execution and Windows behavior were not retested for this follow-up. The macOS Keychain test remains host-blocked and is separately excluded from the remaining workspace suite.

- Final `cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten`: 343 passed, 0 failed, 3 ignored, 1 explicitly filtered Keychain test. The ignored cases are public-download acceptance, live-provider execution and a documentation example.
