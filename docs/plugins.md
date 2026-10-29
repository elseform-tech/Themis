# Plugins and prompt skills

The desktop Integrations screen browses plugin bundles, skills, MCP connections, hooks and marketplace sources. Tile installs and new chat-managed integrations use global app-data scope, with one-click plugin/skill installation and basic lifecycle controls. Existing project integrations remain supported; advanced CLI imports can explicitly choose project scope. Advanced MCP configuration and diagnostics use files or the CLI. Chat management skills and the CLI use the same validated APIs. See [configuration and diagnostics](configuration.md) for locations, precedence, permissions, and logging. Personal plugins have local project scope (`.themis/plugins`) or global app-data scope. App and CLI use the same local server and revisioned store.

## Prompt workflow

Type `/` to choose a skill or `@` to choose an enabled installed plugin. A plugin mention binds its bundle and exposes enabled skill metadata, including manual-only skills, without injecting every instruction body. Skill selection remains explicit. Catalog paths are relative to the run workspace so `read_file` can open the materialized instructions without relaxing file-access boundaries. Type `/create-skill` to ask the agent to design one. The editor renders an icon and skill name; persisted prompts contain `[[skill:ID]]` or `[[plugin:g--NAME]]` / `[[plugin:l--NAME]]`. Disabled or missing plugin mentions fail explicitly. New plugin references use stable scope/plugin/skill identities; plugin version and saved revision remain separate. Existing revision-pinned references still resolve their original instruction snapshot, so updates preserve saved prompts. Current enablement and removal controls apply to old references too. Disabled or uninstalled plugins cause an explicit unavailable-reference error. Old revision references keep their labels but are excluded from new autocomplete selections. Each skill, connection, and hook can be enabled independently. Removing one component preserves its siblings; a later marketplace update can restore components that still exist upstream.

In a chat, Up/Down on the first/last line recalls previously submitted prompts and restores the current draft. Arrow keys select slash suggestions when autocomplete is open. Enter selects a suggestion before it can send a chat; Shift+Enter inserts a newline.

Automations select skills in their task prompt. Continuing automations use the thread's current model and effort, but do not inherit its attached skills. Legacy standalone automation skill IDs remain supported and are migrated into prompt references when edited.

Installed and Discover each retain independent search, source and status filters while the browser is open. Reset filters clears only the current view. Result counts and filtered empty states reflect the current selection. Discover shows cards with descriptions, source, and explicit compatibility status. Click a name to inspect a package. Install on a tile checks compatibility automatically and installs supported packages globally. Partial packages show omissions before Install supported parts; unsupported packages cannot install. A spinner marks progress and the tile retains ✓ Installed on completion. Plugin View lists only skills and their View actions; the separate partial-install decision exposes omissions. Not checked means no compatibility claim. Compatible means the format is supported, not that an external service has been tested. Unsupported packages have no supported capabilities, cannot be installed, and are hidden after inspection unless Unsupported or All statuses is selected. Refresh starts a new persisted compatibility scan. Each skill has one view action that opens the complete instruction body as rendered Markdown. The original `SKILL.md` remains captured byte-for-byte; frontmatter is omitted from the pretty view. Long documents scroll inside the viewer. Public plugin and standalone skill previews use the same validated importer without installing or enabling the package. The list scrolls beneath fixed search controls. Management skills remain available in normal chat; there are no Ask Themis controls. After inspection, compatible packages install with Install; partial packages use Install supported parts with omissions visible before the action. Supported components are installed while unsupported capabilities remain recorded; imported executable components stay disabled. Advanced source/configuration imports, compatibility inspection, MCP tests, and edits use chat or the CLI. Icons use a supplied validated HTTPS or bundled SVG asset; packages without one use a fallback.

## Agent skill creation

The built-in creator owns `crates/core/builtins/create-skill/SKILL.md` and `scripts/verify.py`. Its instructions tell the agent to write a JSON draft, run the materialized verifier at `.themis/skills/create-skill/verify.py`, test helper scripts, then call `save_skill`. Management tools are available in every chat, independently of installed plugin activation. Built-in management skills guide plugin, skill, MCP, hook and automation setup; normal approvals still apply. Global scope is the default for new chat-created skills; edits preserve the existing scope. Backend validation and optimistic revision checks also run on every save.

Run the standalone verifier:

```sh
python3 crates/core/builtins/create-skill/scripts/verify.py /path/to/skill.json
python3 crates/core/builtins/create-skill/scripts/verify.py /path/to/skill-directory
```

It checks required fields, names, instruction/script limits, known tools, duplicate scripts, traversal, and directory symlinks. Skill instruction bodies accept up to 256 KiB of UTF-8 text across import, discovery, creation, and runtime validation; bodies are never truncated to fit. Frontmatter does not count toward the instruction limit. Creator draft files are bounded at 2 MiB to allow JSON escaping and metadata; bundled script content remains capped at 64 KiB. It does not evaluate whether instructions solve the task. Python 3 is required for this check.

## Marketplace compatibility

The default source is Anthropic's `claude-plugins-official` Git repository; individual packages are installed by the user. Additional HTTPS Git repositories or local marketplace directories can be added. Repository imports support an explicit branch, tag, or commit and safe subdirectory. Personal imports accept local skill/plugin folders, a standalone `SKILL.md`, native JSON bundles, MCP JSON files, and pasted MCP JSON. Archives are rejected explicitly. Pasted MCP/native import JSON accepts JSONC comments and trailing commas; runtime and pasted configuration are limited to 1 MiB. File/repository import formats retain their own parser limits; JSONC is not universally supported by upstream manifest files. Imports read Claude marketplace/plugin manifests, `SKILL.md`, UTF-8 resources, `.mcp.json`, and command hooks. Imported MCP servers and hooks begin disabled. Source metadata records local paths, repository refs/subdirectories, or marketplace origin. Upstream icons are retained when they are validated HTTPS URLs or bundled SVG resources; unavailable binary icons are reported. Make a personal copy to customize a marketplace package without losing changes on update.

Unsupported components are prominently listed in the plugin detail view. All import paths reject packages with no supported capabilities; inspection remains available, and existing installed packages are not removed. Claude subagents/LSP/settings and restricted tool matchers are not executed. Plugin folders are read as follows (skill discovery keeps its stricter rules, below):

- Symlinks are followed when their target resolves inside the plugin folder and isn't a folder already being read. Other links (outside the plugin, broken, or looping) are not followed and are listed as "Link not followed: <path>".
- Files over 1 MiB and binary files are left out of the stored resources and listed as notices. Notices, like unused skill frontmatter and Claude tool-name notes, appear in the compatibility details but don't make a plugin "Partially supported".
- When a plugin folder exceeds the 8 MiB / 1,000-file totals (often a whole library or server repository), only the plugin's own folders are imported (`.claude-plugin`, `.mcp.json`, `SKILL.md`, `skills`, `commands`, `agents`, `hooks`, `scripts`, `assets` and declared paths). The plugin is then reported as partial.
- `skills`, `mcpServers` and `hooks` in `plugin.json` or in the marketplace entry may name paths inside the plugin. Skill paths are folders of skills; MCP and hook paths are JSON files that are imported like `.mcp.json` and `hooks/hooks.json`. Paths that are missing or leave the plugin are reported.
- Every `SKILL.md` is a skill, unless that gives two skills the same id. Then only Claude's layout counts (a root `SKILL.md`, `skills/<name>/SKILL.md` and declared skill folders), so copies made for other tools and reference files inside a skill folder are resources, not skills. Ids that still collide come from the skill's folder path (`extra/review` becomes `extra-review`). A plugin with no skills in Claude's layout keeps all of them.
- An MCP server defined identically in `plugin.json` and `.mcp.json` is imported once; different definitions with the same name still fail.
- Claude plugin commands (`commands/**/*.md`, plus command files or folders declared as `commands` in `plugin.json` or the marketplace entry) become manual-only skills, used with `/` like any manual skill. The id is the file name with folders joined by `-` (`commands/git/clean.md` is `git-clean`); a command whose id a skill already uses gets `-command`. The `description` frontmatter, else the first line, is the description. Each command's instructions open with a short note (`builtins/prompts/skill-command-note.md`) saying that `$ARGUMENTS` (and `$1`, `$2`, …) is the text written after the command, that `!`command`` lines are commands to run with the shell tool, and that `@path` names a file to read; those runs and reads go through the normal approvals. Other command frontmatter (`allowed-tools`, `argument-hint`, `model`) is listed as notes.

The importer supports simple scalar/block YAML frontmatter. Unsupported source types are rejected rather than silently ignored. MCP imports accept Claude `mcpServers` and OpenCode `mcp` shapes, normalize local command arrays and HTTP URLs, and require credentials to be `${VARIABLE}` or `{env:VARIABLE}` references; plain non-secret values are allowed as described in [Keys for MCP servers](#keys-for-mcp-servers). Literal credentials and malformed definitions are rejected. Inspection reports unsupported transports/headers, and unknown MCP options with an offending field, reason, and remedy. Advanced strict imports require `--allow-partial` for unsupported components. Catalog installation explicitly offers supported components only for partial packages; skipped capabilities remain recorded. Partial installation never bypasses safety or type validation. Imported connections remain disabled until explicitly enabled. This is compatibility with supported components, not the full Claude plugin runtime.

## Skill discovery

Themis reads conventional user and project `.agents/skills`, `.codex/skills`, and `.claude/skills` folders automatically. User folders are under your home folder: `HOME` on macOS and Linux, `USERPROFILE` on Windows. Runtime `skills.paths` replaces the project search roots only; user discovery continues across the conventional home folders. Discovery does not register or change the source folders. Skills with the same display name receive separate source-path identities; identical canonical sources are deduplicated in `.agents`, `.codex`, then `.claude` order. Source directory symlinks are canonicalized for read-only discovery, cycles are skipped, and symlink resources inside a package are rejected explicitly. Project discovery rejects sources that escape the project root after canonicalization. The scan stops at eight directory levels and 1000 skills. Invalid or oversized skills appear as disabled packages with a compatibility notice. `disable-model-invocation: true` keeps a skill available for explicit slash invocation while excluding it from automatic discovery.

User and project activation overrides persist in Themis. Discovered skills can be disabled; uninstalling their source requires removing the original file. Text reference/script resources are available as per-run copies inside the project sandbox, with no executable permissions granted by discovery. Original absolute `SKILL.md` paths remain available as metadata. Instructions and imported scripts still follow the normal tool approvals when used.

## MCP and hooks

Connections support stdio and Streamable HTTP JSON-RPC, initialization, paginated tool discovery, and tool calls. Optional server-provided initialization instructions reach the model as labelled external tool documentation after startup; they do not grant permissions. Malformed instructions or bodies larger than 32 KiB are rejected. MCP/hook-only plugins have zero bundled skills; Themis does not invent a placeholder `SKILL.md`. Exact legacy generated placeholders are excluded on registry load while authored skills, original resources and revision identities remain preserved. Credentials are never literal configuration values: a server reads keys as `${NAME}` references (see [Keys](#keys-for-mcp-servers)). Remote servers that require OAuth sign-in are supported (below); the legacy SSE transport is not. Enabling a connection yourself (UI or CLI) is the consent to start that exact definition: a changed command, arguments or URL, an agent edit you did not approve, or a definition arriving from a repository or import does not start until you enable it again, and an agent enabling or changing a command-based connection asks you about the exact command every time (an explicit deny rule still blocks startup; see [configuration](configuration.md)); tool calls use Themis approval gates independently of skill selection, and **Allow in this chat** on one tool covers the server's other tools. A failed connection is omitted with a sanitized warning; chat and management remain available to repair it. Blocking hooks retain their failure policy.

Canonical hook events are `RunStart`, `BeforeTool`, `AfterTool`, `BeforeCompaction`, and `RunEnd`; legacy event aliases remain supported. Hook matchers select tools; ignore/abort failure policy is explicit. Commands receive a JSON payload through stdin and may return JSON. Before-event hooks can block on failure. Hooks cannot grant tool permissions. MCP server processes and hooks are not run inside the OS sandbox that confines agent commands (see [sandbox](sandbox.md#what-it-covers)); they run with your permissions once you enable them. Commands have bounded output/time; Unix process groups terminate descendants on timeout/cancellation. Imported resource roots are passed to hooks as `CLAUDE_PLUGIN_ROOT`, never substituted into shell source. Explicit chat/CLI tests run the saved configuration; normal runs request approvals.

## Signing in to remote MCP servers (OAuth)

A remote server that answers with a sign-in challenge (HTTP 401 with `WWW-Authenticate:
Bearer`) shows **Sign in needed** in Integrations › MCP with a **Sign in** button. Themis
follows the MCP authorization spec: it reads the server's protected-resource metadata
(RFC 9728) to find its authorization server and that server's metadata (RFC 8414), registers
itself as a client (dynamic client registration, RFC 7591), and opens the sign-in page in your
own browser with PKCE (S256) and the `resource` parameter (RFC 8707). A one-time listener on
`127.0.0.1` receives the result (five minutes at most); the tokens are stored in the Keychain
(never in configuration files) and refreshed when they expire. **Sign out** forgets them.
Sign-in and sign-out are offered only for servers you installed. A server whose sign-in needs a
client registered by hand (no registration endpoint) is reported as such; use a
`Bearer ${NAME}` header for it instead. A server that has its own key header never signs in. Imported definitions with an `oauth` setting are
accepted; the sign-in happens after installing.

## Keys for MCP servers

A server reads a key as `${NAME}`, the same placeholder MCP documentation uses: in `env` for a
local server (`"env": {"GITHUB_PERSONAL_ACCESS_TOKEN": "${GITHUB_TOKEN}"}`) or in a header for a
remote one, as an optional literal prefix followed by one reference (`"Authorization": "Bearer
${API_KEY}"`, `"X-Api-Key": "Token ${API_KEY}"`, `"CONTEXT7_API_KEY": "${CONTEXT7_API_KEY}"`).
Header names are letters, digits, `-` and `_`; `Accept`, `Content-Type`, `Content-Length`,
`Host` and the MCP session headers are reserved, and `Authorization` may appear once. When the
server starts, each `${NAME}` is the secret named NAME in **Settings › Secrets**, or else the
environment variable of that name (a saved key wins). Keys are stored in the system credential
store (Keychain, Windows Credential Manager, Linux Secret Service, the same store as sign-ins);
app data `keys.json` keeps only the names, because those stores cannot list entries. A server
whose secrets are missing shows **Secret needed** with **Add secret**, which opens Settings › Secrets
on that name; every missing name is reported at once. Settings › Secrets lists each saved secret and each secret
an installed server reads, with the servers that use it and its state (Saved, From environment,
Missing), and offers Add or Replace (a masked field), Remove, and a new name. The secret
OpenCode Go uses (Settings › Models › Providers, `OPENCODE_KEY` by default) is always
listed with "OpenCode Go" among its users, and the secret of each provider in `runtime.jsonc`
with that provider's name, so it works in MCP configuration as `${NAME}` too. Values are never
returned to the UI or written to logs; a server's stderr is redacted of the keys it was given.
Names are letters, digits and `_` (up to 128); values are 1–4096 printable characters without
spaces. The model provider key stays in Settings › Models.

References may also be written `${env:NAME}` or `{env:NAME}`. Claude plugins' install-time settings, `${user_config.option}`, read the secret named after the option in capitals (`${user_config.bigquery_project}` reads `BIGQUERY_PROJECT`), so they are entered in Settings › Secrets like any key. An `env` reference may carry a default, `${NAME:-value}`, used when the secret isn't saved or set; a default must be a plain value as below. A remote server's URL may contain references, including at the start (`${JFROG_URL}/mcp`); it is checked against the HTTPS rule once the references are filled in, when the server starts.

Plain values are allowed where they can't be credentials: `env` values and headers whose name doesn't suggest a credential (no `key`, `token`, `secret`, `password`, `auth`, `cookie`, `session`, `private`, `cert` or similar), at most 256 printable characters, with no run of 20 or more characters mixing letters and digits and no user name or password in a URL. That covers settings such as `"FASTMCP_LOG_LEVEL": "ERROR"` and tags such as `"x-exa-source": "claude-code-plugin"`. URL queries follow the same rule for each parameter (`?client=claude-code-plugin` is fine, `?api_key=…` is refused). `${CLAUDE_PLUGIN_ROOT}` and `${PLUGIN_ROOT}` in the command, arguments or plain `env` values become the installed plugin's folder when the server starts; they are never keys. The same rules apply to imports, Integrations › MCP › Edit as mcp.json and the CLI. Defaults may also appear in URL and header references (`${LOGFIRE_MCP_URL:-https://logfire-us.pydantic.dev/mcp}`).

Imported servers may also carry `description`, `note`, `_note`, `_comment`, `_meta` and `disabled`, which have no effect (imported servers start disabled), and `timeout` and `autoApprove`, which are listed as notes because Themis's own timeouts and approvals apply. A plugin's `userConfig` declaration is noted, since its settings are entered in Settings › Secrets. Preset `oauth` clients and `cwd` are still unsupported.

## Programs MCP servers start with

An app opened from Finder, the Dock or a desktop launcher gets the system's minimal `PATH`.
At startup Themis adds your login shell's `PATH` (three-second limit), so a server whose
command is `npx`, `uvx`, `docker` or a program in `~/.local/bin` starts as it does in your
terminal. A program that still cannot be found is named in the connection details ("'npx'
was not found on this computer"); install it or save the command with its full path.

## Edit as mcp.json (Integrations › MCP)

**Open** writes your personal (non-marketplace) global MCP servers to one copy,
`mcp-edit/mcp.json` in the app data folder, in the standard `mcpServers` shape that Claude
Desktop, Claude Code and Cursor use, keyed `integration/server`: local servers as
`{"command": "npx", "args": [...], "env": {"KEY": "${NAME}"}}`, remote ones as
`{"type": "http", "url": "...", "headers": {"Authorization": "Bearer ${NAME}"}}` (any key header).
The list starts with commented-out examples, one per way to authenticate (a local server with a
key, a remote server with a key header, a remote server that signs in); remove the `//` to use
one. Whether a
server runs is not in the file (switch it in Integrations; an `"enabled"` you add is used),
and opens it in the editor chosen under **Open with** (the same choice as Settings ›
Permissions). While it is open Themis watches it: each save is parsed as JSONC and every
integration whose servers changed is saved through the integrations store with the revision
the copy was made from, exactly as an edit in the CLI. The row then shows **Applied** with the
integrations changed, **Invalid · line N** (nothing is applied), or a **conflict** when that
integration changed elsewhere since the copy was opened (open it again). Adding a server under
an existing personal integration name adds it, and a new name makes a new personal integration;
removing one removes it; a marketplace integration's name is refused. As with any edit, a changed command, arguments or URL does
not start until you enable it again. Marketplace integrations are not listed: make a personal
copy to edit one. The CLI uses the same methods: `open_mcp_config` (with `"open": false` to
only write and watch the copy), `apply_mcp_config` and `mcp_config_status`.

## CLI

```sh
themis plugin list
themis plugin create personal --scope local --project /path/to/project
themis plugin show personal --scope local --project /path/to/project
themis plugin edit personal --scope local --project /path/to/project
themis plugin marketplace list
themis plugin marketplace add community https://github.com/owner/repository.git
themis plugin marketplace browse claude-plugins-official
themis plugin marketplace check claude-plugins-official
themis plugin inspect /path/to/package --scope global
themis mcp inspect /path/to/mcp.json --scope global
themis plugin import /path/to/package --scope global --allow-partial
themis plugin install PACKAGE@claude-plugins-official
themis plugin update PACKAGE
themis plugin disable PACKAGE
themis plugin uninstall PACKAGE
themis skill create THREAD_ID 'Create a global code-review skill'
themis skill verify /path/to/skill.json
themis skill save personal /path/to/skill.json --scope local --project /path/to/project
themis mcp add personal docs /path/to/mcp.json
themis mcp test personal docs --project /path/to/project
themis hook add personal guard /path/to/hook.json
themis hook enable personal guard
themis hook test personal guard --project /path/to/project
```

`mcp` and `hook` also support list, show, update, disable, and delete; skills support list/save/delete. `plugin save`/`import` accept a complete JSON bundle; export writes JSON to stdout. Edits use `$EDITOR` and compare the original revision before saving. Changes create a new revision while active runs preserve their immutable snapshot; changes apply on the next run.

## Verification (2026-10-01)

Fresh local commands and outcomes:

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test --workspace`: hit the host-dependent synthetic Keychain test failure (`The user name or passphrase you entered is not correct`).
- `cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten`: 224 passed, no failures; one Keychain test filtered; live-provider test and illustrative doctest ignored.
- `cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten`: passed; Rust line coverage 83.1% (11,071/13,328 lines). The prior local report was 85.8%; the new plugin runtime lowers aggregate coverage. No frontend coverage report is available.
- `npm --prefix web run lint`, `npm --prefix web test`, `npm --prefix web run build`: passed; 171 frontend tests. Vite still reports chunks above 500 kB and Vitest reports its existing deprecated environment configuration.
- `ruff check quality_dashboard` and `python3 -m unittest quality_dashboard.test_dashboard`: passed, 13 dashboard tests. Product LOC now includes the built-in Python verifier.
- Native debug app bundle built with an isolated `ai.themis.plugins.qa` identifier. Native checks verified Plugins naming, plugin/component persistence, creator autocomplete/inline insertion without sending, automation inline skill save/reopen, and a synthetic hook test. QA automation was saved disabled; no live provider request was made.
- Live CLI browsing of the default official marketplace succeeded; catalog metadata was retrieved without installing a package.
- The CLI plugin E2E drives the actual `themis` binary against an isolated server. The creator request-path integration test captures the mocked LLM request and confirms creator instructions, management tools, and materialized verifier. Python acceptance/rejection and directory validation run as real subprocesses.

The running dashboard was refreshed through `/api/metrics`: 93 AST-analyzed app files; average cyclomatic 2.46, cognitive 2.20, maintainability 73.8. Reviewed new core files: hooks 4.42/3.50/58.5, marketplace 5.93/4.86/66.4, MCP 4.00/4.52/67.9, registry 2.89/1.65/70.1 (cyclomatic/cognitive/maintainability). These are analyzer metrics, not readiness scores. Live agent-generated skill quality, third-party MCP/OAuth behavior, and native history recall during a completed live run remain unverified. History recall has an automated editor regression test.

## Real upstream acceptance (2026-10-06)

Run the network acceptance explicitly:

```sh
cargo test -p themis-core --test plugin_imports real_public_plugin_and_skill_import_acceptance -- --ignored --nocapture
```

The isolated test imports public sources at exact commits; it does not install into the application's user configuration:

- Superpowers `8ca22dba9a94f28898bbce59f2537ff4d87c747d`: 15 skills imported; two compatibility notices; Claude lifecycle matcher/options excluded. Its reviewed upstream session-start script executes through an explicit native `RunStart` adapter and returns valid JSON. Claude-specific context-injection output is not interpreted as a portable hook contract.
- Anthropic skills `683bc88e56f3e09ba94f7055977f3d3aa499f202`, `skills/pdf`: one skill and 12 text resources imported. At this historical check, skill-creator was rejected by the former 32 KiB limit. The current 256 KiB instruction limit accepts its 32,805-byte body; the pinned acceptance test now checks full import.
- Official Claude plugin catalog `d4226d062928f8d9505dbdeadd10217d23361052`: catalog read and `plugin-dev` installation passed; seven skills and nine compatibility notices.
- Official filesystem MCP npm package `@modelcontextprotocol/server-filesystem@2026.8.31`: imported disabled, explicitly enabled, launched, negotiated the supported MCP protocol, discovered 14 tools, and read an isolated marker through the actual `read_file` tool.

These checks verify supported imports and executable integration paths; they do not establish complete compatibility with upstream plugin runtimes, all skill workflows, every OAuth provider, or current MCP specification revisions.

Standalone skills appear under Skills and are invoked with `/`. Plugins contains real plugin packages (including packages with one bundled skill) and uses `@`. Standalone MCP JSON and hooks stay in their respective categories. Imports record `package_kind`; older imports are classified from preserved source metadata/resources without rewriting the source files.

Plugins has Installed and Discover views. Skills lists installed, bundled, and locally discovered skills; it has no separate Discover catalog. Disabled items remain installed. Discover new packages through Plugins. Standalone skills remain available through repository/folder imports and normal chat. Plugin/skill rows use supplied package icons, with file-type glyphs when the source supplies no artwork.

The integration browser uses searchable cards, icons, status labels and readable details. Discover groups marketplace, status and Personal/Public source filters in one toolbar. Catalog installations always use the global user registry and are available to every project. Existing project-scoped integrations remain visible with their scope; advanced CLI imports retain explicit local support. Personal includes local/discovered sources; Public identifies remote repository or marketplace origin, not an independent assertion about repository access permissions. Status filters match the labels: Compatible (green), Partially supported (amber), Unsupported (red) and Check failed (red), and Not checked (neutral). Available includes only Compatible and Partially supported packages; Check failed and Not checked have their own filters; All statuses includes them. Installed filters completed global installations. Installed items filter by Enabled or Disabled. Color always accompanies a text label. Plugin View lists its actual skills and View actions. Skills render as formatted Markdown. MCP details show the complete saved command and observed connection state. Advanced configuration, diagnostics, and execution tests use the shared chat tools or CLI. Basic lifecycle controls use clear on/off switches, public Install buttons and confirmed Uninstall actions. Advanced management also runs through `/manage-plugins`, `/manage-skills`, `/manage-mcp` and `/manage-hooks` in chat, or the CLI. Automations remains separate with Instructions, Repeat and Time, plus optional controls under Advanced.

## Setup and operational status

Installed and Enabled describe storage and activation, not service health. MCP rows show Connecting, Connected, Failed or Disabled. Imports leave executable components disabled; enabling a connection authorizes automatic background initialization. Inspect environment/dependency prerequisites and test connections through the existing chat/CLI workflow. A successful explicit MCP test is evidence for that invocation only. Marketplace scans run in the shared backend and save their results. New marketplaces remain unavailable until every entry has been checked; failed entries show a reason and cannot install. Standalone skill imports use the same validation; bundled skills retain their parent source.

Catalog tiles reuse persisted full-marketplace checks. Scans check up to four packages concurrently per marketplace and save each completed result; navigating away does not cancel them. Compatibility describes import support, not a successful authenticated runtime test. Refresh retries failed checks. Plugin View shows only bundled skills and their View actions; installation limitations appear only when installing a partially supported package. The [design rules](../DESIGN.md) define copy, status colors, filters, and interaction requirements.

## Saved compatibility scans

`scan_marketplace` starts or resumes background scanning and returns `scanning`, `ready`, or `failed`, plus completed/total counts and a scan revision. Adding a marketplace starts its scan immediately. Browsing and manual preview share the same saved result; no plugin tiles become usable until the complete catalog is classified. Individual import failures remain visible as Check failed and disabled Install actions; they do not hide otherwise usable packages. A failed catalog can be retried with Refresh.

Derived metadata and full previews are stored under app-data `compatibility-cache/<marketplace>/`, separately from the revisioned integration registry. They persist across navigation and server restarts. Interrupted work resumes on the next scan/browse/preview request, retaining completed entries. Refresh explicitly replaces the snapshot; source changes and cache schema changes invalidate it. Catalogs are limited to 2,000 uniquely named entries; individual persisted files retain the existing 32 MiB store limit. No MCP server or hook executes during scanning.

A per-marketplace OS lock coordinates scans, preview reads, refreshes, installs and removal. Manual previews and installs wait for the active writer instead of failing at the completion boundary. Repeated scan requests reuse the active job. Removal during scanning returns a retryable busy error. Desktop installs include the reviewed scan revision: if another window refreshed it, installation stops and asks the user to reopen the plugin. The validated saved package is installed, so a later upstream edit cannot change the approved contents. Updating through the CLI explicitly refreshes the full marketplace first. Previously installed packages remain available during scans.

Raising the instruction limit or changing import rules invalidates older saved marketplace checks automatically (the saved check carries a version). `themis plugin marketplace check NAME` waits for a complete check and prints how many plugins have each status (`supported`, `partial`, `unsupported`, `failed`) and how many plugins each reason affects, most common first; `marketplace refresh NAME` fetches the source again first. Existing installed packages and their revisions remain unchanged.

The active Installed/Discover view has a persistent accent underline and a brief selection shimmer (static under reduced motion). Marketplace scanning shows the interactive companion in the center of the content area with a progress ring and completed/total percentage; catalog retrieval is indeterminate until the total is known. Compatibility warnings are collapsed by default with an icon, detail count and amber emphasis. Expand them to read full reasons, offending fields and remedies where available. Rejected local skills remain visible as Unavailable with the validation reason and no activation control.

Failed scans retain the complete validation error chain instead of only the outer component name. Older compatibility snapshots are automatically rechecked for the more informative results.

MCP rows and details show the saved executable and all arguments, with shell quoting for spaces, quotes and empty arguments. Remote entries show their endpoint. Status, activation switch and uninstall action share one compact row. Enabled connections initialize in the background when the app opens or the selected project changes. The app stays usable while each connection starts or fails. Refresh retries failed connections. The shared backend retains transports by project, component and saved revision; chat runs reuse them with their own approval hooks. Connected means initialization and tool discovery succeeded; it does not continuously probe an idle remote service. Local process exit and transport-call failure change the status to Failed.

Transient integration errors and installation confirmations use the shared side toast with its six-second timer and dismiss action. Compatibility details remain attached to the affected item as collapsed warnings.

Background startup is bounded to 30 seconds per connection. Disabled connections never start; explicit user/project approval denials block automatic startup. Enabling authorizes connection setup only, not tool execution. Changes reconcile existing connections; old transports referenced by active runs retain their snapshot until the run releases them. Connections are held in memory by the shared app server, without a second writable registry.

Open an MCP connection’s details to see its exposed tool count, names and descriptions. This reads cached discovery metadata; it does not execute a tool. Tools appear after the connection completes.

Agent inventory (`list_plugins` and `manage_integrations` with `action: "list"`) returns metadata only: `{total, counts: {mcp: {total, enabled}}, items, next_offset}`. It accepts `kind` (`plugin`, `skill`, `mcp`, `hook`), `offset` (default 0), and `limit` (default 25, range 1–100). Totals/counts cover the filtered inventory before pagination; `total` counts packages, while `counts.mcp` counts server definitions. An enabled server requires both its package and component enabled; this does not establish connection health. File bodies, instruction bodies, scripts and MCP environment/command configuration are omitted. Full package inspection remains available through the existing UI/CLI `plugin_action` list API; `plugin_action` with `action: "inventory"` exposes the compact result for local shell processing. Results from inspection or mutation still pass through the shared bounded agent delivery boundary described in [context checkpoints](context-and-attachments.md).
