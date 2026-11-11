# Desktop appearance

ThemisCode keeps its existing workspace and composer. A permanent icon rail contains Plugins, Automations, Review queue, Settings, Help and Search, each with a tooltip. The expandable dock contains the glinting Themis wordmark, a New chat pill, projects and their threads. The top toggle and Cmd/Ctrl+B collapse only that dock; hovering never expands it. Native icons and the favicon remain. The default Classic palette uses neutral greys (frame `#232427`, sidebar `#1c1d20`, canvas `#131314`, surface `#1f2023`, raised `#282a2d`) with the gold accent `#d9a73e` and a warm gradient (`#c08a2e` → `#e3b85a` → `#f6e7bd`) reserved for the greeting, the active-run composer ring, the context warning and review progress.

All new projects are created under the fixed `~/ThemisOS/Projects` root. The default **Themis** project remains first in the sidebar at `~/ThemisOS/Projects/Themis`, without a lock icon. All project display names, including Themis, can be changed without moving their folders or histories. Settings shows only the shared projects root. Settings and creation dialogs offer no location override.

Settings offer 20 presets: **Classic** (the original dark Themis look, and the default), three skies, **Morning**, **Noon** and **Night**, and 16 solid dark presets, including GitHub Dark and GitHub Dimmed. Morning and Noon are light, so the preset decides the light or dark mode (`data-theme`); the separate Light/Dark/System setting stays retired and loads as Dark. The old `system` palette id was renamed `classic` with no alias: it, and any unknown id, shows and loads as Classic. There are 12 locally resolved font choices: System, Arial, Georgia, Mono, Rounded, Avenir, Helvetica, Verdana, Trebuchet, Palatino, Charter and Menlo. Font fallbacks apply where a family is unavailable. Code stays monospace. Size ranges from 12 to 22px.

Themes, fonts and sizes preview throughout the interface before saving. **Apply** persists the draft through the shared server; **Revert**, leaving Settings or a failed write restores the saved appearance. Shared action buttons use unboxed text or icons, and settings labels stay compact.

Overflowing project and thread titles make one leftward pass on hover or keyboard focus, retaining the full accessible title. Reduced-motion mode wraps titles and disables shimmer/motion.

Sidebar SVG paths use Lucide icons; see `docs/lucide-icons-LICENSE`. Native icon generation remains `swift scripts/generate-icon.swift`.

Selected threads shimmer without a colored side stripe. Internal boundaries use continuous layers: a lighter utility/header frame, dark project sidebar and darker canvas. Every palette preserves this brightness order without bright outlines or frame gaps. The layout is framed: the rail sits flush on the frame, and the sidebar dock, the chat stage and the Workspace panel are rounded panels inset by `--themis-panel-gap` (8px), with radii `--themis-stage-radius` (18px) and `--themis-panel-radius` (20px). `--themis-panel-bg` (defaults to the sidebar colour), `--themis-stage-bg` (defaults to the canvas) and `--themis-panel-blur` (`none`) let a theme restyle the panels without touching geometry.

A sky paints `--themis-sky` behind the frame: a translucent frame tint over two soft radial glows over a vertical gradient (Morning `#BFDDFB` → `#DCEBFA` → `#FFE2CC`, Noon `#6DB6F7` → `#A9D6FB` → `#E4F3FF`, Night `#04060E` → `#0A1024` → `#141C44`). Its `glass` sets `--themis-panel-bg` and `--themis-stage-bg` to translucent colours, and `--themis-panel-blur` to `blur(36px) saturate(190%)`, so the sidebar dock and the Workspace panel are frosted glass over the sky. Each sky's opaque colours are stand-ins for that glass over the gradient's middle stop and are what menus, inputs and cards use. The accents of Morning (`#005fc7`) and Noon (`#0052b3`) are darker than the design board's so they reach 4.5:1 on the glass at the brightest sky stop; tests check text, secondary text, accent and the status colours on the glass over every stop, with and without the glows. Light skies use darker status colours (success `#166534`, warning `#92400e`, danger `#9f1239`, info `#0550ae`) and light compatibility-badge colours.

**Fallback.** When the web view does not support `backdrop-filter`, `data-glass="off"` turns the panels solid (the sidebar and canvas colours, no blur) while the sky still shows around them. Blur is off on Linux until measured: WebKitGTK's frame time with the blur has not been measured, so Linux gets the solid panels too. The skies themselves ship on every OS. Appearance swatches show Frame · Sidebar · Canvas in that order; the whole app previews a choice until Apply or Revert. Thread editing changes only its name; the composer controls the next model. Completed responses retain the model captured for their run. Older responses without metadata say “Model not recorded.”

Product rules: [UI rules](ui-rules.md).

Current verification: [UI polish checks](ui-polish-verification.md).

## Historical verification

The dated checks below describe earlier builds, including appearance modes and artwork that have since been removed.

## Local verification (2026-10-01)

- `npm --prefix web test`: 178 tests passed in 22 files, including collapse/navigation, palette/font persistence, contrast and existing composer Enter/IME behavior.
- `npm --prefix web run lint` and `npm --prefix web run build`: passed. Build retains the existing large-chunk warning.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test -p themis-desktop settings:: --lib`: 8 passed, 75 filtered.
- `cargo test -p themis-desktop --test server_cli_e2e appearance_preferences_roundtrip_through_server_and_cli`: 1 passed, 8 filtered. Checks persisted values and rejects unknown fonts without overwriting saved preferences.
- `python3 -m unittest quality_dashboard.test_dashboard`: 13 passed; `ruff check quality_dashboard`: passed.
- `swift scripts/generate-icon.swift`: passed; PNG master is 1024px RGBA, ICO contains a 256px PNG and ICNS includes the macOS sizes.
- Isolated native QA app (`ai.themis.designqa`, Vite frontend) verified expanded/collapsed navigation, saved Light/Dark appearance, palette changes, project creation and retained composer draft/model/effort/send controls. No provider request was sent. Native Serif/22px rendering and saving also passed; drag/window-control interaction remains unverified; production installation was not replaced.

The refreshed dashboard reports mean cyclomatic complexity 2.11, cognitive complexity 1.55 and maintainability 73.2 across 83 app source files. Touched settings/sidebar/app modules were reviewed; no before/after baseline was captured. The existing LCOV report shows 83.1% Rust coverage, but was not regenerated for this change and is not fresh coverage evidence.

Native QA screenshot (synthetic project and unsent draft):

![Framed rail and original composer](images/framed-rail-native.jpg)

## Combined plugin integration verification

The plugin branch is integrated with the Framed rail. The inline skill editor uses the existing composer surface and controls. Shift+Enter keeps its newline behavior; empty skill results do not trap Tab or Enter. Marketplace updates refresh an existing plugin's source, and imported Unix executable resources retain owner-only execution after materialization.

Fresh combined checks: 225 Rust tests passed (2 ignored and the existing named Keychain exclusion), 183 web tests passed, web lint/build and workspace Clippy passed, and all 13 dashboard tests/Ruff passed. The shared-server CLI plugin CRUD and appearance persistence journeys both pass. AST averages after including plugin modules: cyclomatic 2.46, cognitive 2.21, maintainability 73.8 across 94 files; the broader plugin code increases complexity compared with the redesign-only scope. Remote marketplace refreshing is source-verified with local-fixture update coverage; no remote marketplace installation or live provider request was used as acceptance evidence.

Combined native QA also verified local plugin creation, /create autocomplete, inserting a stable skill chip, plain-text paste and Shift+Enter multiline drafting. The Plugins refresh clears transient load errors after a successful reload. IME is covered by automated tests; native IME and window dragging remain unverified.

Hover expansion and its Appearance control are removed. The compact rail remains visible until explicitly expanded; a regression checks pointer entry with old hover-enabled settings, navigation from the rail, and reopening with the top toggle. All 183 web tests, lint/build and 13 dashboard tests/Ruff passed after this correction.

![Compact rail stays visible](images/framed-rail-compact-native.jpg)
