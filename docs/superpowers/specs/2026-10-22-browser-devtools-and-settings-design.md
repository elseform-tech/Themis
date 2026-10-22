# Browser debugging tools and Browser settings

Date: 2026-10-22. Status: design, awaiting approval. Reviewed by a builder check; its questions are answered below.

## Why

The agent already drives a built-in Chromium (`crates/core/src/browser/`), and after each step it sees
uncaught exceptions, `console.error` calls, failed requests and dialogs (`new_since_last_action`). That
was enough to find a planted load-time crash, but not to debug like a developer: it cannot run an
expression in the page, read `console.log`/`warn` output, inspect a request's headers or body, pause
at a breakpoint, look at storage, or profile. The person can open real Chrome DevTools beside the
Workspace view (the `</>` button), but the agent cannot use them.

Separately, browser use is governed today by per-site approval prompts ("Allow once", "Allow in this
chat"), which the person finds an annoyance for a browser that runs in its own profile. They want one
place, a **Browser** section in Settings, that decides what the agent may do, with no prompts.

## Goals

1. The agent can debug a page with the whole DevTools toolset: console, elements, network, sources
   (breakpoints), application (storage, cookies), performance and memory.
2. Debugging is always available on local apps (the apps the agent builds) and available on real
   websites only when the person turns on **Debug real websites**.
3. A **Browser** section in Settings replaces browser approval prompts: browsing is open by default,
   controlled by an editable block list and an optional allow list.
4. A few practical browser settings live there too (on/off, which browser, clear browser data,
   auto-open Workspace).

## Non-goals

- No approval prompts for browser or debugging actions, in Custom or YOLO.
- No per-chat or per-run browser permissions; settings are global (with project restrictions, below).
- No change to the person's own Chrome: the agent's browser keeps its separate profile.
- No changes to `fetch_url`/`web_search` behaviour beyond honouring the block and allow lists.
- No new UI for viewing the agent's debugging output beyond what chat already shows (tool calls and
  results); the person still has real DevTools via `</>`.

## Anti-goals (failure even if it works)

- The agent can be talked by a web page into sending the person's cookies, tokens or passwords
  somewhere. Secrets must be hidden from tool results by default.
- A forgotten breakpoint or a heavy profile hangs a run or heats the machine.
- A repository's own `.themis/config.jsonc` loosens what the person allowed.
- Settings grows into a raw configuration editor or a log viewer (project rule).

## Decisions made with the person

- **Scope:** the whole DevTools toolset, on both local apps and real sites.
- **Real sites:** debugging there is off until **Debug real websites** is turned on in Settings.
- **No approvals:** the setting is the permission; no per-action or per-site prompts once on.
- **Site control by lists:** a block list (pre-filled, editable) and an optional allow list.
- **Defaults** (recommended and accepted): browsing open by default; block list pre-filled with
  sensitive kinds of site; debugging always on for local apps; the allow list, when it has any entry,
  locks the agent to those sites plus local apps.
- **Settings lives in the app:** a dedicated Browser section, an accepted exception to the rule that
  permissions are only edited in the `runtime.jsonc` file (`AGENTS.md` and `docs/ui-rules.md` are
  updated to say so).
- **How far blocking reaches:** the pages the agent opens and acts on (top-level navigation and
  agent actions), not every request a page makes. An allowed page may still load scripts, images and
  sign-in from other hosts (so a local app using Stripe or Google sign-in keeps working).
- **Secret hiding is best-effort**, a guard against accidental exposure, not a security boundary.
  The real protection for sensitive accounts is the block list plus keeping Debug real websites off.
- **Extra settings and concerns** (proposed, accepted): browser on/off, which browser, clear browser
  data, open Workspace when the agent starts browsing; secrets hidden by default; size caps; one heavy
  job at a time; breakpoints auto-resume; projects may only tighten.

## Design

### 1. What counts as local and as a site

- **Local:** `localhost`, `*.localhost`, `127.0.0.0/8`, `[::1]`, `0.0.0.0`, and `file:` pages Themis
  itself opened. (Builder: reuse whatever host classification `checked_url`/`site_of` in
  `crates/core/src/tools/web.rs` already does, extended for these.)
- **Site:** the registrable host as today's site rules use it (a rule for `example.com` covers its
  subdomains).

### 2. Site lists and the debug switch (who may do what)

The lists bind **the agent, not the person**: the person may visit any site in the Workspace
Browser tab; the agent may not read or act on a tab whose current page is blocked (or outside a
non-empty allow list), even if the person opened it. They govern top-level pages only, not the
subresources, iframes or requests a page makes (decided with the person).

They are **enforced in every approval mode, YOLO included**, on their own path in the browser and
web tools, before and independent of the approval hook (`ConfiguredHook::approve` returns early for
YOLO, so it cannot carry this). `AGENTS.md` and `docs/configuration.md` gain one sentence: YOLO
bypasses approval prompts and approval rules, but not the Browser block and allow lists or the debug
switch. (Decided with the person: "blocked means blocked everywhere".)

Resolved per action, for the page's site (or the target of `browser_open`/`browser_navigate`):

1. **Blocked** if the site matches the block list. Blocked means: the agent may not open, navigate
   to, act on or debug it, and `fetch_url`/`web_search` result fetches to it are refused too. The tool
   returns a plain error ("example-bank.com is blocked in Settings → Browser") so the agent tells the
   person rather than working around it. A top-level redirect or navigation into a blocked site
   (for example after a click) is stopped the same way: the tab is returned to `about:blank` and the
   step reports it.
2. Else, if the allow list is **non-empty** and the site is neither local nor on it: refused with
   "only sites on the allow list" in the error.
3. Else **browsing** is allowed with no prompt.
4. **Debugging tools** (section 3) are allowed on local pages always, and on other pages only when
   **Debug real websites** is on.

Pre-filled block list (my call on the exact entries; editable, and a "Restore defaults" control
restores them): common banking and payment sites and government ID services are hard to enumerate
globally, so the pre-fill is a short, honest starter set the person can grow: `paypal.com`,
`stripe.com`, `wise.com`, `revolut.com`, `accounts.google.com`, `login.microsoftonline.com`,
`appleid.apple.com`, `id.me`, `login.gov`, plus the person's email provider is not guessed. The
section says plainly that the list is a starter and not complete.

These lists replace today's per-site browser approvals: `approve_site` is no longer called for
`browser_*` tools; site grants for `browser` in `chat_grants/` are ignored (and dropped on next
write). Existing approval *rules* in `runtime.jsonc` for `browser_*` (allow/deny by site) keep their
current meaning and mode semantics: a deny rule refuses in Custom and is bypassed in YOLO as today
(my call; the Settings lists are the always-on control). Keep `fetch_url`/`web_search` approvals as
they are today, except that the block and allow lists also apply to them: `fetch_url` to a blocked or
non-allowed site is refused; `web_search` itself stays allowed under a non-empty allow list (its
endpoint is infrastructure), and its results are returned unfiltered, with opening or fetching a
result outside the lists refused (my call).

### 3. The debugging tools

One new family of tools, `browser_devtools_*`, small and composable rather than one mega-tool. Each
acts on the chat's current tab unless given `tab`. All results are bounded and redacted (section 4).

| Tool | Does | Notes |
| --- | --- | --- |
| `browser_console` | Runs a JavaScript expression in the page (or the paused frame) and returns its value; or, with no expression, returns recent console messages at all levels | Messages kept per tab from when the tab opened (ring buffer, last 500), with level, text, source location and stack for errors; `since` cursor to read only new ones |
| `browser_elements` | Finds elements by selector or snapshot ref; returns attributes, box, computed styles (chosen properties or a short default set), and outer HTML (capped) | Can set an attribute, style or text, or remove an element, for live experiments; changes are not saved anywhere |
| `browser_network` | Lists recent requests (method, URL, status, type, size, timing); with an id, returns headers and response body (capped); can block URL patterns or throttle (offline, slow 3G, fast 3G) for the tab | Request log per tab, last 300; bodies fetched on demand |
| `browser_debugger` | Sets and removes breakpoints (URL and line, or on exceptions), resumes, steps (over, into, out), and while paused returns the call stack and scope variables | A pause auto-resumes after 30 s unless the agent acts; pausing never blocks other tools' errors from being returned |
| `browser_storage` | Reads and writes cookies, localStorage and sessionStorage for the page's origin; clears them | Values redacted by default (section 4) |
| `browser_performance` | Records a performance trace for N seconds (max 15) or runs a Lighthouse-style audit of the page; returns a summary (long tasks, layout shifts, largest paint, main-thread time by category) | Full trace saved as a file in the chat's media folder; one heavy job at a time per browser |
| `browser_memory` | Takes a heap snapshot or compares two; returns a summary (largest retainers, object counts, growth between snapshots) | Snapshot saved as a file; one heavy job at a time |

The existing `new_since_last_action` stays as is (so ordinary steps still surface errors without a
call). The builder may merge or split these tools if it keeps them small and clearly named; the set
of capabilities is the requirement, not the exact tool count.

Lighthouse: the builder decides whether to bundle an audit or approximate it from the trace and
Performance API; a full Lighthouse bundle is acceptable only if it does not add meaningfully to the
33 MB app.

### 4. Safety inside the tools

Secret hiding is **best-effort** (decided with the person): it stops accidental exposure of
credentials in results and history, not a determined page that has manipulated the agent (an
evaluated expression could encode a token, and open browsing can already send data out by
navigation). The real protection is the block list and keeping Debug real websites off.

- **httpOnly cookies are never returned:** page scripts cannot read them, so the agent's tools do
  not either (names and attributes only), regardless of Show secrets.
- **Secrets hidden by default.** In every tool result: cookie values; `Authorization`, `Cookie`,
  `Set-Cookie`, `Proxy-Authorization` and `X-*-Token`-style headers; storage values and console or
  expression results that look like tokens (JWTs, long base64/hex secrets, `password`/`token`/`secret`
  keys) are replaced with `[hidden: N chars]`. A setting **Show secrets to the agent** (off) turns
  this off. Redaction also applies to what is stored in chat history.
- **Size caps:** expression results and bodies at 16 KiB each in a result; longer values say so and
  point to a saved file when useful; request and console logs bounded as above.
- **Heavy jobs:** one trace or heap snapshot at a time per browser; others wait or fail with "busy".
- **Breakpoints:** auto-resume after 30 s; all breakpoints cleared when the run ends or is stopped.
- **Prompt injection:** results from the page are data, not instructions (same framing the browser
  tools already use). No new network access from Themis itself is added. Requests a page makes are
  not filtered by the lists (navigation-only, above), which is why hiding secrets is best-effort.

### 5. Settings → Browser

A new section beside General, Appearance, Models, Permissions and Advanced. Lean, following
`DESIGN.md` "Only what earns its place". Proposed contents, in order:

1. **Use the built-in browser** (switch; `browser.enabled`).
2. **Browser** (a select of the browsers found on this machine, plus "Choose…" for a path;
   `browser.executable`). Shows "No Chromium browser found" with the install hint when none.
3. **Blocked sites** (an editable list of sites with add and remove; "Restore defaults"). One line
   under it: "The agent never opens these. A starter list, not complete."
4. **Allowed sites** (an editable list; empty by default). One line: "When this has any site, the
   agent may use only these and local apps."
5. **Debug real websites** (switch, off). One line: "Lets the agent use console, network, storage and
   other developer tools on sites other than local apps."
6. **Show secrets to the agent** (switch, off; disabled when Debug real websites is off? Builder's
   call; it applies to local apps too).
7. **Open Workspace when the agent starts browsing** (switch, on).
8. **Clear browser data** (a button, per project chosen from a small menu, or "all projects"): deletes
   the agent browser's profile (logins, cookies, storage) after a confirmation; refused while a run
   uses that browser.

### 6. Storage, precedence and the CLI

- Settings are stored in the user `runtime.jsonc` under `browser` (alongside the existing `enabled`
  and `executable`): `blocked_sites`, `allowed_sites`, `debug_real_websites`, `show_secrets`,
  `open_workspace`. The Settings section edits them through validated store APIs with field-level
  writes (never a raw text editor) and shows file problems as Permissions does today.
  `open_workspace` is a UI preference and may instead live in app settings (`settings.json`) if that
  is simpler; builder's call.
- **Project layer may only tighten:** a project's `.themis/config.jsonc` may add `blocked_sites`, may
  narrow `allowed_sites` (intersection when both set), and may set `debug_real_websites` and
  `show_secrets` to `false`, never `true`. This extends `check_web_restriction` in
  `crates/core/src/configuration.rs`. Arrays from the user file are not replaced by a project file
  (exception to the usual "arrays replace" rule, documented).
- Configuration is captured once per run as today; a change in Settings applies to the next run.
- The CLI and app share it through the app server as today; a CLI end-to-end test covers a blocked
  site, the allow-list lockdown, and debugging refused on a real site with the switch off.

### 7. Documentation and project rules

Update `docs/web-tools.md` (lists, debugging tools, redaction, limits), `docs/configuration.md` (new
fields, precedence exception, project-only-tightens), `docs/ui-rules.md` (Browser section, and the
exception to file-only permissions), `docs/user-guide.md` (what the person can control), and
`AGENTS.md` (Settings may edit Browser fields in place; the rest stays file-only).

## Constraints

- Rust core in `crates/core` (tools in `browser/`, policy in `configuration.rs` and `tools/web.rs`),
  desktop server and Settings in `crates/desktop` and `web/src/screens/Settings.tsx`.
- Chrome DevTools Protocol only (no extra runtime dependencies such as Node or Python).
- macOS, Windows and Linux, as the browser already is.
- Diagnostics log metadata only (tool names, sizes, durations, categories), never expressions,
  values, bodies or URLs with query strings.
- Tests: unit tests per tool against real Chrome in `crates/core/tests/browser_e2e.rs` style; property
  test for site matching and redaction (untrusted input); CLI end-to-end for the policy; web tests for
  the Settings section; a native check of the Settings section.

## Left to the builder

- Exact tool names and argument shapes within the capabilities listed.
- The default computed-style property set; console ring and request log sizes within the stated
  bounds.
- Lighthouse approach (see above).
- Whether `open_workspace` lives in `runtime.jsonc` or `settings.json`.
- Layout details of the Settings section within `DESIGN.md`.
- The order of work, as long as policy (lists and switch) lands before or with the debugging tools.
