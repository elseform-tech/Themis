# Plugins: compatibility, navigation, sources, updates, hooks and icons

Date: 2026-10-29 · Branch: `codex/plugins-design` · Design only; nothing built yet.

Mockups: canvas https://claude.ai/artifact/Em6B9YcQir5EFWhwUN7PEJ, row "Plugins", boards 21–24
(`PluginsDiscover`, `PluginsSources`, `PluginsInstalled`, `PluginsHooks`). Subagents, which also
affect plugins, have their own design: `2026-10-29-subagents-design.md`.

## Why

- Only 105 of the 315 official plugins (33%) fully work. That's the result of a catalog check of
  `main` at `e01a159` on 2026-10-29, run with an isolated HOME. The rest are partial (100),
  unsupported (22) or fail to import (89), and the numbers barely moved since 2026-10-09.
- Discover shows one marketplace at a time in a flat grid. 123 entries share the catalog category
  `development`, and almost no entries carry keywords.
- A new marketplace can't be used until every entry is checked. Nothing tells you about updates,
  and a source can't be removed from the app.
- The Hooks tab lists hooks with no notion of when they run.
- Most plugins show the generic folder icon. HTTPS icons are fetched while browsing.

## Goals

1. Most catalog plugins work as written: about 80% fully working after the compatibility steps
   here, and close to 90% once subagents ship.
2. Discover is grouped by purpose across every marketplace, with one search box.
3. Adding a marketplace is checked up front, and plugins can be installed as soon as their own
   check finishes.
4. Sources stay current without effort. Updating stays an explicit choice.
5. Hooks are shown by event, in the order a run reaches them.
6. Every plugin has a recognisable icon, with no network requests while browsing.
7. UI text is short and professional, with no small hint lines.

## Non-goals

- Running `bin/` executables on PATH, output styles or plugin `settings`.
- LSP servers (12 catalog plugins). They need an LSP client and are a separate project.
- Fetching website favicons.
- A second registry. Everything uses the existing revisioned registry and the
  `compatibility-cache/` store.

## Catalog check numbers

| Step | Plugins that fully work | Share |
|---|---|---|
| Today | 105 | 33% |
| Importer fixes part 1 (P1, measured) | 156 | 50% |
| + part 2 (P2) | about 220 | about 70% |
| + `commands/` as skills (P3) | 239 | 76% |
| + hook events and mapping (P4) | 254 | 81% |
| + subagent profiles (subagents design, step 4) | up to 283 | up to 90% |

- The projections count plugins whose blockers would all go away, assuming each fix handles
  every case of its reason. They are estimates until each step re-runs the check.
- The last row is an upper bound. Many plugin subagents write files, and the subagents design
  runs them read-only unless the main agent gives them paths.
- Plugins whose hook matchers use Claude tool names count as partial until P4 maps them (see P4).

## 1. Compatibility

### P1: catalog check and importer fixes, part 1

- **Catalog check:**
  - A read-only CLI command reports status counts and skip-reason counts for a marketplace,
    built on the existing scan.
  - A regression test runs the importer over a pinned fixture of catalog shapes, and fails when
    an entry that worked stops working.
  - The check also reports official entries that `plugin-groups.json` doesn't cover (P5).
- **Symlinks** (18 plugins fail today):
  - Follow a link whose target, after canonicalising, stays inside the plugin root and isn't a
    folder already being read. Report every other link as "Link not followed: <path>".
  - Skill discovery keeps rejecting symlinks; this change is for plugin packages only.
- **Files over 1 MiB** (18): skip the file with the notice "File too large: <path>" instead of
  failing the plugin.
- **Plugins over the totals** (8 MiB, 1,000 files), usually a whole library or server repository:
  import only the plugin's own folders (`.claude-plugin`, `.mcp.json`, `SKILL.md`, `skills`,
  `commands`, `agents`, `hooks`, `scripts`, `assets` and declared paths) and report the plugin
  as partial.
- **Duplicate skill ids** (10):
  - The cause is the importer counting every `SKILL.md` as a skill. That includes copies made
    for other tools (netlify ships `codex/skills/…`) and reference files inside a skill folder
    (vercel's `upstream/SKILL.md`).
  - Plugins whose skills don't collide keep today's rule, so their ids never change.
  - When ids collide, only Claude's layout counts: a root `SKILL.md`, `skills/<name>/SKILL.md`
    and declared skill folders.
  - Ids that still collide come from the folder path (`extra/review` → `extra-review`).
  - A plugin with nothing in Claude's layout keeps all its skills.
  - No ids need storing in the registry, because ids that import today stay the same.
- **Duplicate MCP servers:** a server defined identically in `plugin.json` and `.mcp.json`
  imports once (CodSpeed, Leadfeeder).
- **Harmless notes no longer make a plugin "partial":** unused skill frontmatter, Claude
  tool-name notes and binary files left out of the text store (images, fonts). That's 55
  plugins. The notes stay listed in the detail view.

### P2: importer fixes, part 2 (MCP)

This step reverses a written rule. `docs/plugins.md` currently requires every environment value
to be a `${VARIABLE}` reference, and that section is updated with this change.

- **Value types:**
  - `McpServer.env` maps a name to a variable today (`connections.rs`).
  - It becomes a value that is either a reference (`${NAME}`, optionally `${NAME:-default}`) or a
    literal.
  - Headers and `args` follow the same type.
- **Literals:**
  - A literal is allowed only if it matches an allow-list of safe shapes: booleans, numbers, short
    lowercase words (`info`, `production`), `localhost` URLs without credentials, and paths inside
    the plugin root.
  - Anything else must be a reference.
  - Defaults in `${NAME:-default}` must pass the same allow-list.
  - One shared validator serves the importer, Integrations › Connections › Edit as mcp.json, and
    the CLI.
  - A property test covers the validator. This affects 11 plugins.
- **`${CLAUDE_PLUGIN_ROOT}`:** resolves to the installed plugin's materialised resource folder,
  the same folder hooks get today. It is filled in by the shared backend when a connection
  starts, so the desktop and the CLI match. Together with defaults, this unblocks 14 plugins.
- **Known harmless fields:**
  - `description`, `_note`, `_comment` and `disabled` are accepted.
  - `cwd` maps when it points inside the plugin root, and `timeout` maps directly.
  - An `oauth` block maps to the existing OAuth sign-in where its fields match.
- **Still refused:**
  - URLs with embedded credentials.
  - Plain HTTP to hosts other than localhost.
  - Relative URLs with no base.
  - SSE.

### P3: `commands/` as skills (39 plugins)

- Each `commands/<name>.md` becomes a manual-only skill named after the command. Its frontmatter
  `description` becomes the skill description.
- `$ARGUMENTS` maps to the text after the slash command.
- `allowed-tools` narrows the tools for the whole run that invokes the command, the same way
  skill tool lists already narrow runs. It never grants permissions.
- **Refused and reported, which makes the plugin partial:**
  - `!` shell expansion inside the command text.
  - `@file` includes.

### P4: hook events, mapping and the hooks view

- **New canonical event `PromptSubmit`**, mapped from `UserPromptSubmit` (9 plugins).
  - **When it runs:** at the start of a turn, after `RunStart` and before the first model request.
    It runs inside the run, so approvals and events are available. The payload includes the
    person's message.
  - **Blocking:** a blocking result stops the turn before any model request, and the chat shows
    "Blocked by hook <name>".
  - **Context:** hook output can add context. This is a new channel to the model, because today
    hook output is only checked for `block`. The context is:
    - wrapped in a prompt file (`hook_context_note.md`) that labels it as external and not
      instructions from the person;
    - capped at 4 KiB;
    - left out of goal checks.
  - It never grants permissions.
- **`PostToolUseFailure`** (2 plugins) maps to the existing `ToolCallFailed` alias of
  `AfterTool`, which only fires when the tool call fails. No new event is needed.
- **Still skipped:** `SessionEnd`, `UserPromptExpansion`, `PermissionRequest` and
  `PostToolBatch`. Each is used by one or two plugins, and `SessionEnd` has no run to attach to.
- **Tool names in matchers:**
  - Imported matchers keep Claude tool names today, so a `Bash` guard never matches Themis's
    shell tool.
  - Matchers are mapped (`Bash` to shell, `Read` to read, `Edit` and `Write` to write and edit,
    `Grep` and `Glob` to search).
  - A matcher name that doesn't map makes the plugin partial and is listed in its detail.
- **Lifecycle options:** `SessionStart` matchers and `PostToolUse` and `Stop` options are mapped
  where Themis has the same meaning, and skipped with a reason otherwise.
- **Hooks view** (board 24): see section 5.

## 2. Discover (P5, board 21)

- **One Discover screen across every visible source:**
  - Source becomes a filter, and each card names its source.
  - Entries are keyed by source and name. The same name in two sources shows as two cards,
    because the code may differ.
- **Groups**, shown as chips with counts: All, Coding, Cloud & deploy, Data & databases,
  Monitoring, Security, Work apps, Design, Other.
  - Language-server plugins, a non-goal, appear under Coding as unsupported.
  - A bundled map (`crates/core/builtins/plugin-groups.json`, keyed by marketplace and plugin
    name) covers the official catalog.
  - Other sources use their catalog `category`, falling back to Other.
- **Search** covers name, description and publisher across all groups.
- **Cards** show the icon, the name, publisher · source, a short description, what the plugin
  adds ("Connects to Airtable · 8 skills") and Install.
  - "Some parts skipped" appears only when it applies. Fully working plugins carry no badge.
- **Default filter:** Compatible, with fully working plugins first and then partial ones.
  Unsupported plugins stay behind the status filter.
- **The last group viewed** is remembered as a per-viewer preference.

## 3. Sources (P6, board 22)

- **Add marketplace:** a single "Source" field that takes `owner/repo`, an HTTPS Git URL or a
  local folder.
  - **Preview first:** before adding, Themis fetches only the manifest and shows its name,
    publisher and plugin count, with "Plugins from this source can run commands once enabled."
    - For a Git source this is a shallow, sparse, blob-filtered clone that checks out only
      `.claude-plugin/marketplace.json`. A local folder is read directly.
  - **Errors are immediate, in one line:** unreachable sources, sources that aren't a marketplace,
    and catalogs over the 2,000-entry limit.
  - **No Name field:** the name comes from the manifest.
- **Install as checks finish:**
  - A checked entry can be installed while the rest of its catalog is still being checked.
    Unchecked entries show "Checking…".
  - An install is tied to that entry's own saved check revision rather than the whole scan's. A
    later re-check of that entry stops the install and asks you to reopen it.
  - `docs/plugins.md` "Saved compatibility scans" changes to match.
- **Sources list:**
  - One row per source: plugin count, when it was last checked, a switch that shows or hides it
    in Discover, and Remove with confirmation.
  - The switch is saved as an `enabled` field in the existing `marketplaces.json`, written under
    the existing marketplace lock.
  - Removing a source keeps the plugins installed from it. Their rows show "Source removed", and
    Update is unavailable for them.
  - The official source can be switched off but not removed.

## 4. Updates and Installed (P7, board 23)

- **Change check:**
  - When the app opens, each source not checked in the last 24 hours is checked for changes.
    - A Git source compares its latest commit (`git ls-remote`).
    - An entry hosted in its own repository compares that repository's ref.
    - A local folder compares a bounded hash of its files, not modification times.
  - Only changed entries are re-checked.
  - An app setting in Settings › General, "Check sources for updates", controls this. It is on by
    default.
- **Update:**
  - An installed plugin whose upstream version or content changed shows the version change
    (`1.5.5 → 1.6.0`) and an Update button. "Update all" sits beside the kind tabs.
  - Updating installs the newly checked package.
  - MCP servers and hooks that are new or changed arrive disabled, as they do today.
- **Installed rows** open to their parts. Each part has its own switch and state (such as
  "Connected"), plus a "1 part skipped" note when that applies.
- **Tabs:** Plugins, Skills, Connections and Hooks.
  - "Connections" replaces "MCP" in the UI.
  - AGENTS.md, `docs/ui-rules.md` and `docs/plugins.md` are updated to match.
  - The mcp.json editor and the CLI keep the MCP name.

## 5. Hooks by event (P4, board 24)

- **Order:** the Hooks tab lists canonical events in the order a run reaches them:
  1. Prompt submitted
  2. Run starts
  3. Before a tool runs
  4. After a tool runs
  5. Subagent starts and Subagent ends (once subagents ship)
  6. Before context is compacted
  7. Run ends
- **Empty events** are dimmed.
- **Hook rows** show the name, source and tool matcher. "Blocks on failure" appears when the
  policy is abort, "Main and subagents" appears on tool events once subagents ship, and each row
  has its own switch.
- **Skipped hooks** get one line ("1 hook skipped (release-notes): unsupported event").
- **No activity counts.** Showing how often each hook ran would need its own store, which isn't
  worth it yet.
- **Editing:** apart from the switches the view is read-only. Adding and editing hooks stays in
  the CLI and `/manage-hooks`.

## 6. Icons (P8)

- **Resolved in order:**
  1. **The plugin's own icon:** a bundled SVG, PNG or WebP up to 256 KB, or an HTTPS URL.
     - HTTPS icons are downloaded once, during the compatibility check, never while browsing.
     - All icons are stored as bounded binary assets in a content-hashed cache under
       `compatibility-cache/<marketplace>/icons/`, separate from the text resource store.
     - Installed plugins copy their icon next to their materialised resources.
  2. **A bundled set of brand logos** (Simple Icons, public-domain SVGs), matched only on the
     marketplace `author` or the GitHub owner of the source, never on an MCP domain. The set is
     limited to publishers in the catalog, and the build checks it stays under 300 KB.
  3. **A tile** with the plugin's initials on a colour derived from its name.
- **No network requests while browsing.**

## Configuration and diagnostics

- **App setting:** "Check sources for updates" in Settings › General, on by default.
- **Per-viewer preference:** Discover's remembered group.
- **`marketplaces.json`:** gains a per-source `enabled` field.
- **No new `runtime.jsonc` fields.**
- **Diagnostics, metadata only:**
  - Catalog check summaries: counts per status and per reason.
  - Source added, removed and refreshed: duration and outcome.
  - Update available and update installed: plugin name and versions.
  - Never file contents, manifests or MCP values.

## Order of work

| PR | Content | Depends on |
|---|---|---|
| P1 | Catalog check, importer fixes part 1 | — |
| P2 | Importer fixes part 2 (MCP value types) | P1 |
| P3 | `commands/` as skills | P2 |
| P4 | `PromptSubmit`, matcher mapping, the hooks view | P3 |
| P5 | Discover: groups, all sources, cards | P1 |
| P6 | Sources: preview, install as checks finish, Sources list | P1 |
| P7 | Updates and Installed rows | P6 |
| P8 | Icons and the binary asset cache | P1 |

P1 through P4 each re-run the catalog check and record the numbers in the PR.

## Verification

- **Core:**
  - Unit and property tests for symlink containment (including loops), the literal-value
    validator, skill-id selection, the `commands/` converter, matcher mapping and manifest
    path resolution.
  - A fuzz target for the `marketplace.json` and `plugin.json` parsers, per `docs/fuzzing.md`.
- **Shared server/CLI end-to-end:**
  - Import fixtures for each fixed reason.
  - A `PromptSubmit` hook that blocks, and one that adds capped, labelled context.
  - Add a local marketplace, install while it's still being checked, remove the source and keep
    the install.
  - The change check, then an update.
- **Web and app:**
  - Tests for groups, the source filter, Sources, Installed rows and the hooks view.
  - A run in the real desktop app with an isolated HOME, installing and using at least one plugin
    for each fixed reason.
- **Catalog check:** run against the official catalog after each of P1 to P4, labelled as a
  benchmark audit.

## Open questions

- Whether "Check sources for updates" should also install updates automatically for plugins with
  no MCP servers or hooks. Proposed answer: no, updates stay explicit.
