# Themis product design

These are product rules for new work and UI review. They describe the intended experience; they are not a claim that every existing screen already complies. Preserve working workflows while correcting violations. Screen-specific behavior lives in [UI rules](docs/ui-rules.md); dated verification belongs in `docs/verification/`.

## Philosophy

Make the work, its state, and the next useful action obvious. Keep the interface quiet. Every visible word, container, button, badge, and animation must help someone understand or complete the current task. Remove anything that does not.

- Prefer direct actions and familiar controls over explanations of how the interface works.
- Show useful content first. Reveal detail when requested or when a decision needs it.
- Preserve necessary permission, error, scope, and destructive-action information. Brevity must not hide consequences.
- Reuse existing components, spacing, and theme tokens. Do not add a second visual language for a new feature.
- A beautiful static screen is insufficient: loading, empty, error, disabled, selected, focused, and completed states must work too.

## Copy: no unnecessary text

- Use short, concrete labels: **Install**, **View**, **Refresh**, **Personal**, **Public**.
- Do not repeat the heading, selected tab, control label, status, or action in nearby helper text.
- Omit introductory paragraphs, promotional language, redundant subtitles, implementation jargon, and permanent success explanations.
- Add helper text only when it prevents a likely mistake. Place it beside the decision it explains.
- Empty states need one short explanation and an action only when an action can resolve them.
- Errors say what failed and how to recover. Put raw fields, import diagnostics, logs, and stack traces in advanced workflows.
- Do not show `lspServers`, “Not included,” or “No supported capabilities to install in Themis” as routine plugin detail content. Use a concise compatibility label on the tile; explain limitations at an installation decision when needed.
- Keep skill instructions complete and readable. Reducing interface copy must never truncate the requested content.

## Only what earns its place

This applies to every screen, not just Settings. Before adding an element, ask what it tells this person about this task that they cannot already see; if nothing, leave it out.

- Show what varies or needs attention, not facts that are always true. Fixed explanations (what a feature covers, how a mechanism works, which backends exist on other systems) belong in the docs.
- Show a status only when it is not the expected one. No "Loaded", "OK" or "On" badges for the normal case; show "Invalid", "Not created", "Off" or "Not available" when that is the case.
- One fact, one place. Do not add summary lines ("+2 from this project") that repeat what each row already says, or a banner that repeats an inline error.
- Hide empty sections instead of rendering "Nothing yet", unless the empty state needs an action.
- Prefer a plain phrase to machine detail: "This project and temporary folders", not the absolute temp paths. Show a path only when the person needs it to act (a file to edit).
- Describe the current state, not every option: show the active mode with its one-line meaning, not a glossary of all modes.
- Avoid ornamental numbering, badges and labels; order and grouping already carry meaning.

## Layout and containers

- Use whitespace, alignment, typography, and surface tone for hierarchy before adding borders or boxes. Settings is the exception: its groups of rows sit in quiet rounded containers so each section reads as a unit.
- A container must group a meaningful unit. Avoid nested cards, boxed headings, decorative panels, and a separate box for every sentence or status.
- Tiles are useful for browsing comparable integrations: recognizable icon, name, concise purpose, compatibility, and direct action. Keep repeated fields aligned.
- Use lists for compact sequential content such as a plugin's skills. Do not turn each skill into a dashboard card.
- Group related dropdowns in one filter row. Installed and Discover keep independent search and filter selections. Search belongs with the collection it filters; filters use the same terms as visible labels.
- Keep content readable at narrow widths and at all supported text sizes. Wrap descriptions and controls rather than clipping actions or creating page-wide horizontal scrolling.
- Preserve the continuous frame → sidebar → canvas layers, with descending brightness. Avoid bright frame outlines and gutters.

## Buttons and interaction

- Prefer unboxed text or icon actions. A stronger treatment is reserved for the primary decision or a destructive action.
- One action gets one obvious entry point in its immediate context. Avoid duplicate plus buttons, repeated Install buttons, or redundant navigation controls.
- Icon-only controls require an accessible name and, where useful, a short tooltip. Their hit area must remain comfortable even when the icon is small.
- Use native buttons, selects, inputs, and keyboard behavior. Keep visible focus indicators; do not rely on hover to reveal the only way to act.
- Disable duplicate submissions while an action is pending. Show a spinner only while work is actually pending, then a stable result such as **Installed**.
- Never use an endless spinner to mean “installed,” “enabled,” or “ready.” These are distinct states.
- Preserve drafts, selection, focus, and scroll when unrelated state changes. Dialogs move focus inside, support Escape, and restore it to the trigger.
- Destructive actions identify what will be removed. Avoid confirmation for ordinary reversible browsing actions.

## Integration browsing

- Complete marketplace compatibility checks in the background before publishing its tiles. Save results across visits; manual views reuse them. Bound background work and ignore stale responses after source changes.
- Compatibility means the import format is supported. It does not prove credentials, external services, or runtime execution work.
- Use **Compatible**, **Partially supported**, **Unsupported**, **Checking…**, or **Check failed** honestly. Do not silently turn failure into success.
- Show compatibility reasons on the tile. Collapse warnings by default to a summary with an icon and detail count. Expand to wrapped full reasons, offending fields and remedies when available; use amber emphasis. Do not require hover or motion to read them.
- Render full skill instructions in a restrained inset reading surface, with distinct Markdown headings, lists, inline code, and syntax-highlighted code blocks. Do not flatten the document into a monotone paragraph or add decorative editor controls.
- Keep Install on the tile. New UI installations are global; preserve existing project installations and their scope.
- Opening a plugin shows its skill list and View actions. Viewing a skill opens the full instructions with a way back. A package with no skills gets only a concise empty state.
- Keep descriptions, compatibility explanations, MCP inventories, hook inventories, and installation controls out of the plugin's skill-list view.
- Show partial-install limitations only in the installation decision. Unsupported items remain discoverable through status filters and cannot offer a misleading install action.
- Keep Personal/Public and status filters together. Public denotes a remote catalog/repository origin, not a guarantee of repository access or vendor endorsement.
- Installation, activation, authentication, and successful execution are separate facts. Imported MCP connections and hooks start disabled; never use an Installed badge as proof of a working connection.

## Color system

Dark mode is the product baseline; the Morning and Noon skies are the light looks alongside it, and each preset carries its own `mode`. Palette values come from [`web/src/theme/appearance.ts`](web/src/theme/appearance.ts), with CSS defaults in [`web/src/theme/tokens.css`](web/src/theme/tokens.css). Use the tokens rather than copying hex values into new components. Appearance presets may change neutral surfaces and accents; semantic status meaning must stay consistent.

| Role | Token | Classic preset / value | Rule |
| --- | --- | --- | --- |
| Frame | `--themis-shell-bg` | `#0e0f10` | Persistent app chrome; sits furthest back, the panels stand off it |
| Sidebar | `--themis-sidebar-bg` | `#262729` | Project and chat navigation |
| Canvas | `--themis-canvas` | `#212225` | Main working surface (the stage) |
| Surface / raised | `--themis-surface`, `--themis-raised` | `#2a2b2f`, `#323438` | Inputs and meaningful elevation |
| Border | `--themis-border` | `#3a3c41` | Necessary boundaries only |
| Primary text | `--themis-text` | `#e6e6e6` | Content and primary labels |
| Secondary text | `--themis-text-2` | `#a8abb2` | Useful supporting information; never a reason to add filler |
| Accent | `--themis-accent` | `#d9a73e` | Sparse emphasis, not decoration on every control |
| Success | `--themis-success` | `#81c995` | Successful outcome |
| Warning | `--themis-warning` | `#e3b85a` | Limitation or decision requiring attention |
| Danger | `--themis-danger` | `#f4968d` | Failure or destructive action |
| Information | `--themis-info` | `#8ab4f8` | Neutral informational emphasis |
| Sky | `--themis-sky` | frame colour (solid presets) | Sky presets only: frame tint, glows and gradient behind the frame |
| Glass | `--themis-panel-bg`, `--themis-stage-bg`, `--themis-panel-blur` | sidebar, canvas, `none` | Sky presets: translucent panels on every OS, `blur(36px) saturate(190%)` only on macOS; solid under `data-glass="off"` (reduced transparency) |
| Panel edge | `--themis-panel-highlight`, `--themis-panel-ring`, `--themis-panel-shadow`, `--themis-stage-edge` | `rgba(255,255,255,.05)`, `transparent`, `rgba(0,0,0,.28)`, `#34363b` | Composed into `--themis-panel-sheen` and `--themis-panel-edge` (side panels) and `--themis-stage-edge-shadow` (stage); never replaced by an opaque border |
| Shadow | `--themis-shadow` | `rgb(0 0 0)`; light `rgb(8 20 45 / .35)` | Mix with transparent at each shadow's strength |
| Warm gradient | `--themis-gradient-1/2/3` | `#d0a660` → `#e3b85a` → `#f6e7bd` | Approved Themis moments only (below) |

Light skies (`data-theme="light"`) keep the same roles with darker status colours that reach 4.5:1 on the sky-blue frame and the pale glass: success `#11522a`, warning `#78340b`, danger `#8d0f33`, information `#034394`. Shadows use the soft navy `--themis-shadow`. The warm gradient becomes a deep bronze-to-amber ramp (`#4d2c02` → `#5c3604` → `#693e00`) so the greeting, the wordmark glint, shimmer and the working spinner stay readable; context-warning and compaction text use `--themis-warn-text` (`#f6e7bd` on dark, `#5c3604` on light).

Compatibility badges use the `--themis-ok-*`, `--themis-warn-*` and `--themis-bad-*` tokens (ink, background, line) from [`tokens.css`](web/src/theme/tokens.css), dark / light. The recording pill uses the `bad` pair too. Keep this mapping consistent across tiles and filters.

| Label | Text | Background | Border |
| --- | --- | --- | --- |
| Compatible | `#8cdbab` / `#21633d` | `#193c29` / `#e7f5ec` | `#3b7350` / `#a5cfb4` |
| Partially supported | `#f3cc7c` / `#795009` | `#41351c` / `#fff4d9` | `#816833` / `#ddc18c` |
| Unsupported / Check failed | `#f3a1aa` / `#a02e3a` | `#48252c` / `#fcecef` | `#884b55` / `#e4afb7` |
| Unchecked / disabled | Secondary text token | Neutral surface | Border token |

- Color reinforces a written status; it must never carry meaning alone.
- Keep ordinary text contrast at least 4.5:1, large text at least 3:1, and essential control/focus boundaries at least 3:1 against adjacent colors. Verify actual rendered combinations across presets.
- Do not reuse warning or danger colors for arbitrary category decoration. Personal/Public are provenance labels, not good/bad ratings.
- Avoid gradients, glow, shimmer, or saturation as generic decoration. Approved treatments: the drifting warm-gradient greeting and faint glow on an empty chat, the shimmer on the composer branch chip while it is hovered or focused, the sweeping ring on the composer while a run is active, the warm context-meter warning near auto-compact, status shimmer for real pending work, and the wordmark glint. Honor reduced motion for all of them.

## Typography, motion, and accessibility

- Respect the selected local font and supported 10–22px text size (12 by default; the smallest labels are about 11px at that size). Code remains monospace.
- Use a restrained heading hierarchy and comfortable reading width. Skill content should not stretch across a wide window.
- Keep full accessible names for truncated titles. Reveal the complete title on keyboard focus as well as hover; reduced motion wraps it.
- Animation communicates a transition or real work. No artificial delays, invented progress percentages, or repeated focus-stealing window activation.
- Announce meaningful asynchronous results without continuously reading decorative updates.
- Test with keyboard navigation, enlarged text, reduced motion, long names, empty data, slow requests, failed requests, and narrow windows.

## Review gate

Before calling a UI change complete:

1. Remove words, controls, badges, sections and containers that do not support the current task (see **Only what earns its place**).
2. Verify state labels and actions against the actual backend behavior.
3. Exercise the changed journey in a refreshed browser or native app, including a failure or empty state where relevant.
4. Check focus, keyboard access, contrast, overflow, and loading/completion feedback.
5. Run relevant automated checks and record failures, skips, and unverified journeys separately from passes.

These rules guide implementation and review. They do not replace actual runtime verification or authorize changing unrelated workflows.

Compatibility scans belong to the backend. Save and reuse results across visits. Show the Loader (the Themis wordmark and a thin progress bar with the actual completed/total; a sweeping indeterminate bar until the total is known) centered in the content area while a marketplace is checked, keep navigation usable, and expose its tiles only after the entire scan completes. Manual previews and installs reuse that snapshot. A refresh must invalidate older pending views and installation approvals, and duplicate clicks must never create duplicate operations.

- Transient integration errors and confirmations use the shared timed side toast, never loose inline notices. Persistent compatibility limitations stay in collapsible item details. MCP uses compact installed rows with aligned status/actions and the complete saved command.

- Enabled MCP connections initialize independently in the background on app entry and project selection. Display observed connection status beside activation; failures use timed toasts and expandable details, and never block the workspace. Refresh retries failures.
