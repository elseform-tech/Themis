# Browser debugging tools and Browser settings

Date: 2026-10-22. Status: design, awaiting approval. A builder check reviewed an earlier draft; its
questions are answered below. The permissions model was then changed, at the person's request, to
follow OpenAI Codex's built-in browser.

## Why

The agent already drives a built-in Chromium (`crates/core/src/browser/`), and after each step it sees
uncaught exceptions, `console.error` calls, failed requests and dialogs (`new_since_last_action`). That
was enough to find a planted load-time crash, but not to debug like a developer: it cannot run an
expression in the page, read `console.log`/`warn` output, inspect a request's headers or body, pause
at a breakpoint, look at storage, or profile. The person can open real Chrome DevTools beside the
Workspace view (the `</>` button), but the agent cannot use them.

Browser use is governed today by per-site approval prompts ("Allow once", "Allow in this chat"),
which the person finds an annoyance. They want one place, a **Browser** section in Settings, modelled
on OpenAI Codex's built-in browser (Settings › Browser: browser data, history, downloads, agent
permissions with defaults and per-site exceptions, and Developer mode › "Enable full CDP access";
https://learn.chatgpt.com/docs/browser?surface=app, read 2026-10-22).

## Goals

1. The agent can debug a page with the whole DevTools toolset: console, elements, network, sources
   (breakpoints), application (storage, cookies), performance and memory.
2. A **Browser** section in Settings decides what the agent may do in its browser, Codex-style: a
   master switch for developer tools, default permissions for browsing, downloads and debugging
   (each Allow, Ask or Block), and per-site exceptions.
3. Downloads work (today they are blocked), into the system Downloads folder by default.
4. The browser's own history and data are manageable there: search and remove history, clear
   browsing data by time range and type.

## Non-goals

- No file uploads by the agent (Codex does not automate them either); stated plainly in the docs.
- No confirmation before "sensitive actions" (submitting, purchasing, deleting) beyond the
  permissions below; the person chose permissions over per-action prompts.
- No change to the person's own Chrome: the agent's browser keeps its separate profile; no import of
  cookies, passwords or history from another browser.
- No camera, microphone, location or notification access for pages: always denied, no setting.
- No new UI for viewing the agent's debugging output beyond what chat already shows; the person
  still has real DevTools via `</>`.

## Anti-goals (failure even if it works)

- The agent is talked by a web page into sending the person's cookies, tokens or passwords
  somewhere, through anything Themis could reasonably have hidden.
- A forgotten breakpoint or a heavy profile hangs a run or heats the machine.
- A repository's own `.themis/config.jsonc` loosens what the person allowed.
- Settings grows into a raw configuration editor or a log viewer (project rule).

## Decisions made with the person

- **Scope:** the whole DevTools toolset, on local apps and real sites.
- **Permissions model: Codex's, in full.** Defaults for Browsing, Downloads and Debugging, each
  Allow / Ask / Block, plus per-site exceptions; this replaces the earlier block and allow lists.
- **Developer mode switch:** "Enable full CDP access" turns the debugging tools on or off as a whole.
- **Downloads on by default**, into the system Downloads folder, can be turned off.
- **History, cookies and clearing data** are included (asked for, matching Codex).
- **How far blocking reaches:** the pages the agent opens and acts on (top-level navigation and
  agent actions), not every request a page makes; an allowed page may still load scripts, images and
  sign-in from other hosts.
- **Secret hiding is best-effort:** a guard against accidental exposure, not a security boundary.
- **Settings lives in the app:** a dedicated Browser section, an accepted exception to the rule that
  permissions are only edited in the `runtime.jsonc` file.
- **Other accepted items:** browser on/off, which browser, open Workspace when browsing; secrets
  hidden by default; size caps; one heavy job at a time; breakpoints auto-resume; projects may only
  tighten.

## Design

### 1. Local and sites

- **Local:** `localhost`, `*.localhost`, `127.0.0.0/8`, `[::1]`, `0.0.0.0`, and `file:` pages Themis
  itself opened; `about:blank` and `data:` pages count as local. Local pages are always Allow for
  every permission (they are the apps the agent builds), still subject to the Developer mode switch.
  (Builder: extend the host classification `checked_url`/`site_of` in `crates/core/src/tools/web.rs`.)
- **Site:** the registrable host, as today's site rules use it; an exception for `example.com`
  covers its subdomains, and a more specific exception (`pay.example.com`) wins over a broader one.

### 2. Agent permissions (who may do what)

Three permissions, each with a default and optional per-site exceptions, each value Allow, Ask or
Block:

| Permission | Covers | Default |
| --- | --- | --- |
| Browsing | open, navigate, click, type, press, select, scroll, snapshot, screenshot, record, tabs | Allow |
| Downloads | a download started by the page or by an agent action | Allow |
| Debugging | the debugging tools of section 3 | Block (my call; see Developer mode) |

**Developer mode › Enable full CDP access** (switch): off means the debugging tools are not offered
to the agent at all, on any page, local included, and the Debugging row is shown disabled. On means
the Debugging permission decides. Default **on** (my call: debugging the apps the agent builds is
the main use, and real sites stay Block under it; Codex ships it off). The person may flip it.

**Pre-filled exceptions** (Browsing: Block; editable; "Restore defaults" restores them; a short
honest starter set, not complete, and the person's email provider is not guessed): `paypal.com`,
`stripe.com`, `wise.com`, `revolut.com`, `accounts.google.com`, `login.microsoftonline.com`,
`appleid.apple.com`, `id.me`, `login.gov`. Lockdown is expressed as Browsing default Block plus Allow
exceptions.

**Resolution per action**, for the page's site (or the target of `browser_open`/`browser_navigate`):
local → Allow; else the most specific matching exception's value for that permission; else the
default. Then:

- **Allow:** proceed, no prompt.
- **Block:** refuse with a plain error naming the setting ("Debugging is blocked for example.com in
  Settings › Browser") so the agent tells the person instead of working around it. A top-level
  navigation or redirect into a Browsing-Blocked site is stopped: the tab returns to `about:blank` and
  the step reports it. A Downloads-Blocked download is cancelled and reported.
- **Ask:** an approval prompt with **Allow once**, **Always allow on this site** (writes an Allow
  exception for that permission and site into Settings, visible and removable there) and **Deny**.
  This replaces today's browser "Allow in this chat" grants and the per-site `approve_site` path for
  `browser_*`; existing `browser` site grants in `chat_grants/` are ignored and dropped on next write.

**Modes:** in YOLO, Ask becomes Allow (no prompts); Block always holds, and so does the Developer
mode switch. Enforcement therefore runs on its own path in the browser and web tools, before the
approval hook (`ConfiguredHook::approve` returns early for YOLO). `AGENTS.md` and
`docs/configuration.md` gain one sentence: YOLO bypasses approval prompts and approval rules, but not
the Browser permissions' Block or the Developer mode switch.

**Scope of blocking** (decided): the lists govern the pages the agent opens and acts on, not
subresources, iframes or requests a page makes.

**Who it binds:** the agent, not the person. The person may visit any site in the Workspace Browser
tab; the agent may not read or act on a tab whose current page is Browsing-Blocked for it, even if
the person opened it.

**Existing approval rules** in `runtime.jsonc` for `browser_*` keep their current meaning as an
extra deny layer in Custom (bypassed in YOLO as today; my call). **Web tools:** `fetch_url` to a
Browsing-Blocked site is refused; `web_search` stays allowed (its endpoint is infrastructure), its
results are not filtered, and fetching a result is governed as above (my call).

### 3. The debugging tools

Small, composable tools, offered only when Developer mode is on. Each acts on the chat's current tab
unless given `tab`; all results are bounded and redacted (section 4). The capability set is the
requirement; the builder may merge or split tools if they stay small and clearly named.

| Tool | Does | Notes |
| --- | --- | --- |
| `browser_console` | Runs a JavaScript expression in the page (or the paused frame) and returns its value; with no expression, returns recent console messages at all levels | Per-tab ring of the last 500 messages with level, text, source location and stack for errors; a `since` cursor reads only new ones |
| `browser_elements` | Finds elements by selector or snapshot ref; returns attributes, box, computed styles (chosen properties or a short default set) and outer HTML (capped) | Can set an attribute, style or text, or remove an element, for live experiments; nothing is saved |
| `browser_network` | Lists recent requests (method, URL, status, type, size, timing); with an id, returns headers and response body (capped); can block URL patterns or throttle (offline, slow 3G, fast 3G) for the tab | Per-tab log of the last 300 requests; bodies fetched on demand |
| `browser_debugger` | Sets and removes breakpoints (URL and line, or on exceptions), resumes, steps (over, into, out); while paused returns the call stack and scope variables | A pause auto-resumes after 30 s unless the agent acts |
| `browser_storage` | Reads and writes cookies, localStorage and sessionStorage for the page's origin; clears them | Redacted by default; httpOnly values never returned |
| `browser_performance` | Records a trace for N seconds (max 15) or runs a Lighthouse-style audit; returns a summary (long tasks, layout shifts, largest paint, main-thread time by category) | Full trace saved as a file in the chat's media folder |
| `browser_memory` | Takes a heap snapshot or compares two; returns a summary (largest retainers, object counts, growth) | Snapshot saved as a file |

`new_since_last_action` stays as is. Lighthouse: bundle an audit only if it does not add meaningfully
to the 33 MB app; otherwise approximate from the trace and Performance API (builder's call).

### 4. Safety inside the tools

Secret hiding is **best-effort** (decided): it stops accidental exposure in results and history, not
a determined page that has manipulated the agent (an evaluated expression could encode a token, and
browsing can already send data out by navigation). The real protection is Debugging: Block on real
sites (the default) and Browsing: Block exceptions for sensitive accounts.

- **httpOnly cookies are never returned** (names and attributes only), whatever Show secrets says,
  since page scripts cannot read them either (my call).
- **Secrets hidden by default:** cookie values; `Authorization`, `Cookie`, `Set-Cookie`,
  `Proxy-Authorization` and `X-*-Token`-style headers; storage values and console or expression
  results that look like tokens (JWTs, long base64/hex secrets, keys named `password`, `token`,
  `secret`) become `[hidden: N chars]`. **Show secrets to the agent** (off) turns this off. Applies
  to what is stored in chat history too.
- **Size caps:** 16 KiB per expression result or body in a result; longer values say so and point to
  a saved file when useful.
- **Heavy jobs:** one trace or heap snapshot at a time per browser; others fail with "busy".
- **Breakpoints:** auto-resume after 30 s; all cleared when the run ends or is stopped.
- **Prompt injection:** page content is data, not instructions (as the browser tools already say).

### 5. Downloads, history and browsing data

- **Downloads:** allowed per the Downloads permission; saved to the system Downloads folder by
  default, or the folder chosen in Settings; a name clash gets a numbered suffix; the tool result and
  the chat show the saved path. Size cap per download (builder's call, at least 500 MB) and no
  automatic opening. The download is not attached to the chat.
- **History:** the browser keeps its own history per project (it already does in its profile).
  Settings › Browser can search it, reopen a page in the chat's browser, and remove entries; the agent
  gets no new history tool (not asked for).
- **Clear browsing data:** pick a time range (last hour, day, week, all time) and types (history;
  cookies and site data, which signs the agent out; cached files), for one project or all; refused
  while a run uses that project's browser.

### 6. Settings › Browser

A new section beside General, Appearance, Models, Permissions and Advanced, lean per `DESIGN.md`.
In order:

1. **Use the built-in browser** (switch; `browser.enabled`).
2. **Browser** (select of browsers found on this machine plus "Choose…"; `browser.executable`).
3. **Agent permissions:** three rows (Browsing, Downloads, Debugging), each a select of Allow, Ask,
   Block; under them **Site exceptions**, a list of sites with their values, add and remove, and
   "Restore defaults".
4. **Developer mode › Enable full CDP access** (switch, on) with one line on what it allows.
5. **Show secrets to the agent** (switch, off).
6. **Downloads folder** (the path, Change…, Reset to system default).
7. **Open Workspace when the agent starts browsing** (switch, on).
8. **History** (opens a small searchable list with Reopen and Remove).
9. **Clear browsing data…** (time range, types, project; confirmation).

### 7. Storage, precedence and the CLI

- Stored in the user `runtime.jsonc` under `browser` (beside the existing `enabled` and `executable`):
  `developer_mode`, `permissions` (`browsing`, `downloads`, `debugging` defaults), `site_exceptions`
  (site → per-permission values), `show_secrets`, `downloads_directory`, `open_workspace`. The Settings
  section edits them through validated store APIs with field-level writes that keep the file's
  comments (never a raw text editor), and shows file problems as Permissions does. `open_workspace`
  may live in app settings instead (builder's call). History and browsing data stay in the profile.
- **Projects may only tighten:** a project's `.themis/config.jsonc` may lower defaults (toward Block),
  add Block exceptions, and turn `developer_mode` and `show_secrets` off, never the reverse; its
  exceptions add to the user's rather than replacing them (a documented exception to "arrays replace").
  Extends `check_web_restriction` in `crates/core/src/configuration.rs`.
- Captured once per run as today; "Always allow on this site" takes effect immediately for the
  current run as well as later ones.
- CLI end-to-end tests cover a Browsing-Blocked site, lockdown, Debugging blocked and Developer mode
  off, Ask resolved in YOLO, and a download landing in a chosen folder.

### 8. Documentation and project rules

Update `docs/web-tools.md`, `docs/configuration.md`, `docs/ui-rules.md`, `docs/user-guide.md` and
`AGENTS.md` (Settings may edit Browser fields in place; Browser permissions' Block and Developer mode
hold in YOLO; Workspace and approval wording for the new Ask prompt).

## Constraints

- Rust core in `crates/core` (tools in `browser/`, policy in `configuration.rs` and `tools/web.rs`),
  desktop server and Settings in `crates/desktop` and `web/src/screens/Settings.tsx`.
- Chrome DevTools Protocol only; no Node or Python at runtime.
- macOS, Windows and Linux.
- Diagnostics log metadata only, never expressions, values, bodies or URLs with query strings.
- Tests: real-Chrome tests per tool (`crates/core/tests/browser_e2e.rs` style); property tests for
  site matching and redaction; CLI end-to-end for the policy; web tests and a native check for the
  Settings section.

## Left to the builder

- Exact tool names and argument shapes within the capabilities listed.
- Default computed-style properties; ring and log sizes within the stated bounds.
- Lighthouse approach; download size cap (at least 500 MB).
- Where `open_workspace` lives; Settings layout details within `DESIGN.md`.
- Order of work, as long as the permissions land before or with the debugging tools.
