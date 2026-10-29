# Plugins: compatibility, navigation, sources, updates, hooks and icons

Date: 2026-10-29 · Branch: `codex/plugins-design` · Design only; nothing built yet.

Mockups: canvas https://claude.ai/artifact/Em6B9YcQir5EFWhwUN7PEJ, row "Plugins", boards 21–24
(`PluginsDiscover`, `PluginsSources`, `PluginsInstalled`, `PluginsHooks`). Subagents, which also
affect plugins, have their own design: `2026-10-29-subagents-design.md`.

## Why

- Only 105 of the 315 official plugins (33%) fully work. This comes from a catalog check of `main`
  at `e01a159` on 2026-10-29, run with an isolated HOME. The rest are partial (100), unsupported
  (22) or fail to import (89). The numbers barely moved from the 2026-10-09 check.
- Discover shows one marketplace at a time, as a flat grid with no grouping. 123 entries share the
  catalog category `development`, and almost no entries carry keywords.
- A marketplace you add can't be used until every entry is checked. Nothing tells you about
  updates, and a source can't be removed from the app.
- The Hooks tab lists hooks with no notion of when they run.
- Most plugins show the generic folder icon.

## Goals

1. Most catalog plugins work as written. The target is about 80% fully working after the
   compatibility steps here, and about 90% once subagents ship.
2. Discover is grouped by purpose across every marketplace, with one search box.
3. Adding a marketplace is checked up front, and its plugins can be installed as their checks
   finish.
4. Sources stay current without effort. Updating stays an explicit choice.
5. Hooks are shown by event, in the order a run reaches them.
6. Every plugin has a recognisable icon without fetching anything while you browse.
7. UI text is short and professional, with no small hint lines.

## Non-goals

- Running `bin/` executables on PATH, output styles or plugin `settings`.
- LSP servers (12 catalog plugins). That needs an LSP client and is a separate project.
- Website favicons fetched at runtime.
- A second registry. Everything uses the existing revisioned registry and the
  `compatibility-cache/` store.

## Catalog check numbers

| Step | Plugins that fully work | Share |
|---|---|---|
| Today | 105 | 33% |
| Importer fixes (P1, P2) | 220 | 70% |
| + `commands/` as skills (P3) | 239 | 76% |
| + hook events and mapping (P4) | 254 | 81% |
| + subagent profiles (subagents design, step 4) | 283 | 90% |

The projections count only plugins whose blockers would all go away, assuming each fix handles
every case of its reason. They are estimates until each step re-runs the check.

## 1. Compatibility

### P1: catalog check and importer fixes, part 1

- **Catalog check:**
  - A CLI command reports status counts and skip-reason counts for a marketplace. It is built on
    the existing scan and stays read-only.
  - A regression test runs the importer over a pinned fixture of catalog shapes. It fails when an
    entry that worked stops working.
- **Symlinks** (18 plugins fail today):
  - Follow links whose target resolves inside the plugin root.
  - Reject links that escape the root, and links that loop.
  - Discovery already does this for skill folders.
- **Files over 1 MiB** (18): skip the file and record "File too large: <path>" instead of failing
  the whole plugin. The 8 MiB and 1,000-file plugin totals stay.
- **Duplicate skill ids** (10): prefix the id with the plugin name only when ids collide.
- **Manifest paths:**
  - Read `skills` paths declared in `plugin.json` (33), external MCP config files (18) and external
    hook config files (5).
  - Each path is resolved inside the plugin root, under the same escape checks.
- **Harmless notes count as compatible:**
  - Unused skill frontmatter, Claude tool-name notes and dropped binary images no longer make a
    plugin "partial". This affects 55 plugins.
  - They stay listed in the detail view.
- **Icons:** accept bundled PNG and WebP up to 256 KB, as well as SVG.

### P2: importer fixes, part 2 (MCP)

- **Environment values:** support `${VAR:-default}` and `${CLAUDE_PLUGIN_ROOT}` (14 plugins fail
  today).
- **Plain values:**
  - Allow plain non-secret values in `env` and headers, such as `LOG_LEVEL=info` or
    `Accept: application/json`.
  - Values whose name or shape looks like a credential (`*_KEY`, `*_TOKEN`, `*_SECRET`,
    `Authorization`, bearer-like strings) must still be `${NAME}` references. This affects 11
    plugins.
  - A property test covers the classifier.
- **Known harmless fields:** accept `description`, `_note`, `_comment` and `disabled`. Map `cwd`
  (inside the plugin root) and `timeout`. `oauth` blocks are mapped to the existing OAuth sign-in
  where they match it.
- **Still refused:**
  - URLs with embedded credentials.
  - Plain HTTP to non-local hosts.
  - Relative URLs with no base.
  - SSE.

### P3: `commands/` as skills (39 plugins)

- Each `commands/<name>.md` becomes a manual-only skill named after the command. Its frontmatter
  `description` becomes the skill description.
- `$ARGUMENTS` maps to the text after the slash command. `allowed-tools` only narrows tools and
  never grants permissions.
- The prompts that describe commands follow the prompt rules in AGENTS.md.

### P4: hook events and mapping, and the hooks view

- **New canonical events:**
  - `PromptSubmit`, mapped from `UserPromptSubmit` (9 plugins). It runs before the run starts and
    can add context but never permissions.
  - `ToolFailed`, mapped from `PostToolUseFailure` (2).
  - `SessionEnd`, which fires when a chat is closed or deleted (2).
  - `UserPromptExpansion`, `PermissionRequest` and `PostToolBatch` stay skipped. Each is used by
    one or two plugins.
- **Mapping:** lifecycle hooks with matchers and hook options (`SessionStart` matchers,
  `PostToolUse` and `Stop` options) are mapped where Themis has the same meaning, and skipped with
  a reason where it doesn't.
- **Hooks view** (board 24): see section 5.

## 2. Discover (P5, board 21)

- **One Discover screen across every enabled source.** Source becomes a filter, and each card
  names its source. A plugin listed in several sources shows once; its detail lists the other
  sources.
- **Groups**, as chips with counts:
  - All, Coding, Language servers, Cloud & deploy, Data & databases, Monitoring, Security, Work
    apps, Design, Other.
  - A bundled map (`crates/core/builtins/plugin-groups.json`, plugin name → group) covers the
    official catalog. Otherwise the catalog `category` is used, and failing that, Other.
  - The catalog check (P1) reports official entries the map doesn't cover.
- **Search** covers name, description and publisher across every group.
- **Cards** show:
  - The icon, name and publisher · source, plus one short description.
  - What the plugin adds ("Connects to Airtable · 8 skills").
  - Install.
  - "Some parts skipped" appears only when it applies. Fully working plugins carry no badge.
- **Default filter:** Compatible, which means fully working plugins first, then partial ones.
  Unsupported plugins stay behind the status filter.
- **Layout:** the Discover | Installed switch replaces today's per-view controls. Discover
  remembers the last group, per viewer.

## 3. Sources (P6, board 22)

- **Add marketplace:** one field, "Source", that accepts `owner/repo`, an HTTPS Git URL or a local
  folder.
  - The name comes from the manifest, so there's no Name field.
  - Before you add it, Themis reads only `marketplace.json` and shows the name, publisher and
    plugin count, with the line "Plugins from this source can run commands once enabled."
  - Unreachable sources, non-marketplaces and catalogs over the 2,000-entry limit fail right
    away, with a one-line error.
- **Installing as checks finish:**
  - A checked entry can be installed while the rest of its catalog is still being checked.
    Unchecked entries show "Checking…".
  - Install binds to that entry's saved check revision instead of the whole scan's revision. A
    later re-check of that entry stops the install and asks you to reopen it, as the whole-scan
    revision does today.
  - `docs/plugins.md` "Saved compatibility scans" changes accordingly.
- **Sources list:** one row per source, with its plugin count, when it was last checked, a switch
  that shows or hides it in Discover, and Remove (with confirmation).
  - Removing a source keeps the plugins you installed from it.
  - The official source can be switched off but not removed.

## 4. Updates and Installed (P7, board 23)

- **Change check:**
  - When the app opens, any source not checked in the last 24 hours gets a check of its latest
    commit (`git ls-remote`, or the folder's modification times).
  - If it changed, only the entries whose files changed are re-checked.
  - An app setting in Settings › General, "Check sources for updates", controls this and is on by
    default.
- **Update:**
  - An installed plugin whose upstream version or content changed shows its version change
    (`1.5.5 → 1.6.0`) and Update. "Update all" sits beside the kind tabs.
  - Updating installs the newly checked package.
  - MCP servers and hooks that are new or changed arrive disabled, as they do today.
- **Installed rows open to their parts:** each part has its own switch and state (for example
  "Connected"), plus a "1 part skipped" note when that applies.
- **Tab names:** Plugins, Skills, Connections (MCP servers) and Hooks.

## 5. Hooks by event (P4, board 24)

- The Hooks tab lists canonical events in the order a run reaches them:
  1. Run starts
  2. Before a tool runs
  3. After a tool runs
  4. Subagent starts and Subagent ends (once subagents ship)
  5. Before context is compacted
  6. Run ends
- Events with no hooks are dimmed.
- **Each hook row shows:**
  - Its name, source and tool matcher.
  - "Blocks on failure" when its policy is abort.
  - "Main and subagents" for tool events, once subagents ship.
  - Recent activity ("1 block today", "Ran 3× today"), from bounded run metadata.
  - Its switch.
- One line lists skipped hooks ("1 hook skipped (release-notes): unsupported event").
- The view stays read-only apart from the switches. Adding and editing hooks stays in the CLI and
  `/manage-hooks`.

## 6. Icons (P8)

Icons are resolved in this order:

1. The plugin's own icon: an HTTPS URL, or a bundled SVG, PNG or WebP up to 256 KB.
2. A bundled set of brand logos from Simple Icons (public-domain SVGs), matched by publisher. The
   publisher comes from the marketplace `author`, then the GitHub owner of the source, then the
   MCP server's domain.
3. A tile with the plugin's initials on a colour derived from its name.

There are no network requests while browsing. The bundled set is limited to publishers in the
catalog and stays under a 300 KB budget, which the build checks.

## Configuration and diagnostics

- **App setting** in Settings › General: "Check sources for updates" (on).
- Discover's remembered group is a per-viewer preference.
- No new `runtime.jsonc` fields.
- **Diagnostics (metadata only):**
  - Catalog check summaries: counts per status and reason.
  - Source added, removed and refreshed: duration and outcome.
  - Update available and update installed: plugin name and versions.
  - Never file contents, manifests or MCP values.

## Order of work

| PR | Content | Depends on |
|---|---|---|
| P1 | Catalog check, importer fixes part 1 | — |
| P2 | Importer fixes part 2 (MCP) | P1 |
| P3 | `commands/` as skills | P2 |
| P4 | Hook events, mapping and the hooks view | P3 |
| P5 | Discover: groups, all sources, cards | P1 |
| P6 | Sources: checked add, install as checks finish, Sources list | P1 |
| P7 | Updates and Installed rows | P6 |
| P8 | Icons | P1 |

P1 through P4 each re-run the catalog check and record the numbers in the PR.

## Verification

- **Core:**
  - Unit and property tests for symlink containment, the secret-value classifier, the `commands/`
    converter and manifest path resolution.
  - A fuzz target for the `marketplace.json` and `plugin.json` parsers, per `docs/fuzzing.md`.
- **Shared server/CLI end-to-end:**
  - Import fixtures for each fixed reason.
  - Add a local marketplace, install while checks are still running, remove the source and keep
    the install.
  - The change check followed by an update.
- **Web:** tests for groups, the source filter, Sources, Installed rows and the hooks view. Then
  a run in the real desktop app with an isolated HOME, installing and using at least one plugin
  from each fixed reason.
- **Catalog check** against the official catalog after each of P1 through P4, labelled as a
  benchmark audit.

## Open questions

- Whether "Check sources for updates" should also install updates automatically for plugins with
  no MCP servers or hooks. The proposal: no, updates stay explicit.
