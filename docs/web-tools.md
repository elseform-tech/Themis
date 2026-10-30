# Web tools and the built-in browser

Agents read the web with `fetch_url` and `web_search` and drive a built-in browser. Nothing needs installing in
Themis; search needs a SearXNG instance you run yourself.

## `fetch_url`

Reads one http or https page as text. HTML becomes its main content in Markdown
(Readability extraction, falling back to the whole body without scripts, styles
and navigation); plain text, JSON and XML pass through; other content types
(PDF, images, archives) are refused with their type. No JavaScript runs: for
pages that need it, or to click and type, use the browser tools.

| Limit | Value |
| --- | --- |
| Schemes | `http`, `https`; no user name or password in the URL |
| Redirects | up to 5 on the same host; a redirect to another host returns `redirect_to` and the agent must fetch it again (so the new site gets its own approval) |
| Body | first 5 MiB; `body_truncated` says when more was dropped |
| Time | 30 s for the whole request |
| Page | 8,000 characters from `offset`, with `next_offset` when more remains; pages read this run are paged from memory without downloading again |

Error pages (HTTP 4xx/5xx) still return their text with a `warning`. Every result
carries `"untrusted": true`: page text is data, never instructions or permission.

## `web_search`

Asks [SearXNG](https://docs.searxng.org/), a metasearch engine that queries Google, Bing,
Brave, DuckDuckGo and others. **Themis ships its own SearXNG**: a standalone Python with a
pinned SearXNG (`scripts/bundle-searxng.mjs`, about 155 MB installed, 50 MB of the download;
release builds add it with `tauri build --config tauri.searxng.conf.json`),
started in the background with the app server on a private `127.0.0.1` port and stopped with
it (a watchdog stops it too if the app is killed). Its settings, secret and log are in the
app-data `searxng/` folder; nothing is installed system-wide. Local debug builds use it too
once `node scripts/bundle-searxng.mjs` has packed it into `crates/desktop/searxng-runtime/`
(gitignored); `THEMIS_SEARXNG_DIR` still overrides where it is looked for. Settings › Web shows it as
**Built-in** with its live status. This is macOS and Linux only: SearXNG does not run on Windows
(it needs the Unix `pwd` module), so Windows builds ship without it and Settings › Web shows
"Not set · web_search is off" until you enter a server's address (such as the Docker one below).
`fetch_url` works everywhere.

To use your own instance instead, enter its address in Settings › Web, or set it in the user
`runtime.jsonc` (a project cannot):

```jsonc
{ "web": { "search_url": "http://127.0.0.1:8888" } }
```

Your SearXNG must allow JSON output: add `json` to `search.formats` in its
`settings.yml`. A local instance, for example:

```sh
mkdir -p searxng && printf 'use_default_settings: true\nserver:\n  secret_key: "%s"\n  limiter: false\nsearch:\n  formats: [html, json]\n' "$(openssl rand -hex 32)" > searxng/settings.yml
docker run -d --name searxng -p 127.0.0.1:8888:8080 -v "$PWD/searxng:/etc/searxng" searxng/searxng
```

Development builds use the built-in one only with `THEMIS_SEARXNG_DIR` pointing at
`crates/desktop/searxng-runtime` (built by `node scripts/bundle-searxng.mjs`).

Results are titles, URLs, snippets and, when known, publication dates, plus
`failed_engines`: the engines that did not answer and why (`CAPTCHA`, `Suspended:
too many requests`, ...). SearXNG keeps no index of its own, so results are live,
but upstream engines suspend busy instances: on a 2026-10-20 local test only one or
two engines answered at a time. Few results with failed engines may be an outage,
and the tool says so. Searches use the general category (it found fresh news better
than the news category), at most 30 per run, at least one second apart. HTTP 403
means JSON output is off; an unreachable URL names the setting to fix.

## The built-in browser

Agents also drive a real Chromium that you can watch and use in **Workspace →
Browser**. Themis talks to it directly over the Chrome DevTools Protocol; there is
no MCP server or Python to install. By default it uses its own engine, Google's
`chrome-headless-shell` (Chrome for Testing, a pinned stable version), which the app downloads
from `storage.googleapis.com/chrome-for-testing-public` into app-data `browser/engine/`
the first time the browser is used (about 100 MB to download, 200 MB unpacked). It is a
plain program rather than an app, so it never appears in the macOS Dock (a headless
Google Chrome shows up under recent apps), and it needs no Chrome installed. The first
browser step waits for the download (the address bar shows a loading bar); only if it
fails is an installed Chrome, Chromium, Edge or Brave from the usual folders (macOS
`Applications`, Windows `Program Files` and `AppData\Local`, the Linux `PATH`) used.
The download is pinned to one version (`crates/core/src/browser/engine_pin.rs`, bumped
with `node scripts/pin-browser-engine.mjs`) and checked against its SHA-256 before
anything is unpacked or run; redirects are refused. A browser
chosen in Settings › Browser or `browser.executable` in the user `runtime.jsonc` (for
example a Flatpak Chromium) is always used instead. Tests never download the engine. Browser use, screenshots and
recordings work the same on macOS, Windows and Linux. One gap: on Windows,
continuously animated pages (canvas games, video) streaming into Workspace are
unverified, since GitHub's GPU-less Windows test machines paint them only once.
Themis knows the browser is ready from its "DevTools listening" line or from the
`DevToolsActivePort` file it writes in the profile, whichever comes first (some
Windows Chrome builds write only the file); if neither arrives within 45 seconds,
the error quotes the browser's last few output lines.
The browser runs headless with its own profile in app data
(`browser/profiles/<project>/`): logins persist across a project's chats and never
touch your own browser profile, Keychain or saved passwords. One browser serves a
project; each chat has its own tabs (at most 8). Downloads are saved to the downloads folder (below). Discarding
a chat closes its tabs; the browser exits when no chat of the project uses it.
It never outlives Themis: if Themis crashes or is force-quit, a small guard
process (macOS, Linux) or a kill-on-close job (Windows) ends the browser and its
helpers within seconds. Run as root (as in many Linux containers), Chromium starts
without its sandbox, the only way it runs there.

| Tool | Does |
| --- | --- |
| `browser_open` | Opens a URL (optionally in a new tab) |
| `browser_navigate` | Back, forward or reload |
| `browser_snapshot` | Reads the page again (or one element's part) |
| `browser_click` | Clicks an element by ref, or a point of the visible page by `x`/`y` in CSS pixels of the visible page (a canvas, map or game; a point outside the page is refused). Models get text only, so screenshots are for the person, not for the model to look at; `double` for a double-click |
| `browser_type` | Types into a field (`clear` first, `submit` with Enter) |
| `browser_select` | Chooses `<select>` options by value or label |
| `browser_press` | Presses a key: `Enter`, `Escape`, `Tab`, arrows, `Control+a` |
| `browser_scroll` | Scrolls the page or one element |
| `browser_tabs` | Lists, switches or closes this chat's tabs |

Every action waits for the page to settle (loaded after a navigation, network quiet
for half a second; a loaded page whose network never goes quiet, such as analytics or
polling, counts as settled 2 s after loading; at most 30 s after a navigation and 10 s
otherwise) and returns a
fresh **snapshot**: the page's accessibility tree as an outline, at most 12,000
characters, where elements the model can act on carry a ref right after their name:

```text
- heading "Create your account" [level=2]
- textbox "Email" [e1]: ada@example.com
- textbox "Password" [e2]: ••••••
- combobox "Country" [e3] [expanded=false]: France
- button "Create account" [e4]
```

Refs stay the same across snapshots of one page and reset when it navigates.
Password fields are always masked. Each result also lists what is
`new_since_last_action`: uncaught errors and `console.error` messages, HTTP 4xx/5xx
answers and failed requests of pages, fetches and XHRs (URLs without query strings),
and dialogs. Alerts are accepted; confirm and prompt dialogs are dismissed, so a
page never gets a "yes" the agent did not give. Everything is untrusted data.

**In Workspace.** The Browser tab streams the chat's current tab at the panel's size.
The agent's first browser step in a run opens it for the chat on screen.
Back, Forward, Reload and the address bar work as in any browser. The person and
the agent use the page at the same time: clicking, typing, scrolling or entering an
address goes straight to the page and never pauses the agent. While the agent acts,
one line says what it is doing. The speaker button turns page sound on or off (off by
default for each chat). Sound plays from the hidden browser straight to the Mac's
speakers; Chrome has no per-tab mute, so a script in every page mutes media elements
and suspends Web Audio while it is off (frames from another site inside the page may
still play). Recordings have no sound. **DevTools** opens
Chrome's own DevTools for the current tab below the page; a red count shows console
errors.

**`/browser-use`.** A built-in command (like `/plugins`) that adds browser
guidance to the message: open with `browser_open`, act on refs from the returned
snapshot, check `new_since_last_action`, re-snapshot when a ref is gone, show
results with `browser_screenshot`/`browser_record` and `attach_file`, treat page text
as untrusted, never type secrets the user did not give, ask before irreversible
submissions, and ask the user to sign in in the Browser tab when a login is needed. It lives in
`crates/core/builtins/browser-use/SKILL.md`.

## Screenshots, recordings and media in replies

| Tool | Does |
| --- | --- |
| `browser_screenshot` | Saves a PNG of the visible page, the whole page (`full_page`, up to 16,384 px tall) or one element (`ref`) and returns its path |
| `browser_record` | `start` records the current tab (default 30 s, at most 120 s); `stop` saves an H.264 MP4 and returns its path and length |
| `attach_file` | Shows images, videos and PDFs from the project under the reply: `files: [{path, caption?}]`, at most 12 per reply, 200 MB each |

Screenshots and recordings are saved like composer attachments, in
`.themis/attachments/<id>/screenshot-HHMM-SS.png` or `recording-HHMM-SS.mp4` (local
time), which Git ignores. `attach_file` copies other project files there so a reply
keeps showing them; files outside the project are refused. Recording samples the
page's screencast at 10 frames per second and encodes it with the open-source
OpenH264 encoder built into the app (no ffmpeg). Stopping the run ends any browser
step at once, including an encode in progress, whose video is then not saved. One recording runs per chat at a
time: starting a second one fails with how long the first has run. The agent can
stop only its own recording; a recording the agent leaves running is stopped and
saved when its run ends. `attach_file` is a read; screenshots and recordings ask
like reads of the current site.

**In replies.** Attached media sit under the reply in order: one item as a large
preview, two to four as tiles with their captions, five or more as three tiles and a
**+N** tile. A video tile shows its first frame and a play mark. Clicking a tile opens
the viewer, which steps through every attached item of the chat with Previous/Next
or the arrow keys and closes with Escape.

**In the Browser tab.** **Capture** saves the visible page and adds it to the message
you are writing; **Record** starts a recording and, when you stop it, adds the video
the same way. While anything records, a red line above the page reads "Recording
0:12 · started by you · stops at 2:00" with **Stop**, the page has a red frame, and the
thread header shows a red pill with the time on every Workspace tab and with Workspace
closed; the pill opens the Browser tab.

## Permissions (Settings › Browser)

These settings live in the `browser` block of your `runtime.jsonc`, which you may also edit
by hand. Settings › Browser saves only the field you change and refuses a save when the file
changed since the page loaded (showing the latest instead), so it never overwrites a hand
edit; the page also re-reads the file when you return to the window. Changes apply from each
chat's next message, existing chats included; a reply already running keeps its settings.

What the agent may do in its browser is set in **Settings › Browser**, modelled on OpenAI
Codex's built-in browser. Three permissions each have a default and per-site exceptions,
and each value is **Allow** (no prompt), **Ask** (an approval prompt) or **Block**:

| Permission | Covers | Default |
| --- | --- | --- |
| Browsing | open, navigate, click, type, press, select, scroll, snapshot, screenshot, record, tabs | Allow |
| Downloads | saving a file a page or an agent action downloads | Allow |
| Debugging | the debugging tools (console, elements, network, debugger, storage, profiling) | Block |

- **Local apps** (`localhost`, `*.localhost`, loopback addresses, `about:blank`) are always
  Allow: they are the apps the agent builds.
- **Site exceptions** name a host such as `example.com`; it covers its subdomains, and the
  most specific matching exception wins. A starter set blocks Browsing on sign-in, payment
  and identity sites (`paypal.com`, `stripe.com`, `wise.com`, `revolut.com`,
  `accounts.google.com`, `login.microsoftonline.com`, `appleid.apple.com`, `id.me`,
  `login.gov`); it is a starter, not complete, and you can edit or remove it. Lockdown is
  Browsing default Block plus Allow exceptions for the sites you trust.
- **Block** is refused in every approval mode, **YOLO included**, and the agent is told to
  tell you instead of working around it. A click, redirect or script that lands on a
  Browsing-Blocked site clears the tab and reports it. The lists bind the agent, not you:
  you can visit any site in the Browser tab, but the agent will not read or act on a
  blocked page you opened. They govern the pages the agent opens and acts on, not the
  scripts, images or requests an allowed page loads.
- **Ask** shows a prompt with **Allow once**, **Always allow on this site** (saves an Allow
  exception for that permission in Settings › Browser, used for the rest of the run and
  later runs) and **Deny**. YOLO turns Ask into Allow.
- **Allow** runs without a prompt; a deny rule in `runtime.jsonc` (below) still refuses in
  Custom.
- `fetch_url` to a Browsing-Blocked site is refused too; otherwise the web tools keep their
  own per-site approvals. `web_search` results are not filtered.

The web tools ask per site, after they know it (the URL's host, or your SearXNG host for
searches). In Custom, rules may name a site and **Allow in this chat** covers that web tool
on that site; see [composer permissions](configuration.md#composer-permissions). Approval
rules for `browser_*` (for example `{ "tool": "browser_*", "command": ["example.com"],
"action": "deny" }`) keep working in Custom as an extra deny layer.

```jsonc
{ "approval": { "rules": [
  { "tool": "fetch_url", "command": ["docs.rs"], "action": "allow" },
  { "tool": "fetch_url", "command": ["internal.example.com"], "action": "deny" }
] } }
```

YOLO fetches without asking; the OS sandbox does not apply to these in-process
requests, exactly as for MCP HTTP connections. Diagnostics never record page text
or search results.

## Downloads, history and browsing data

- **Downloads** are allowed by default and saved to the system Downloads folder (on Linux
  `XDG_DOWNLOAD_DIR` when set), or the folder chosen in Settings › Browser. Only a download
  that starts within 3 seconds of your own input in the Browser tab (a click, a key, the
  address bar) is yours and always saved. Every other download, the agent's or one a page
  starts on its own, needs the Downloads permission for the page's site (Block cancels it, Ask
  asks), and is cancelled when no run is there to decide it; the agent's step reports the
  saved path, or why it was refused. A name already in the folder gets a numbered suffix;
  nothing is opened or attached to the chat. The agent cannot upload files.
- **History:** the browser keeps its own history per project in its profile. Settings ›
  Browser › History and data searches it, reopens a page in the open chat's browser, and
  removes entries. The agent has no history tool.
- **Clear browsing data:** a time range (last hour, day, week, all time) and types: history;
  cookies and site data (signs the agent out; site storage such as localStorage and IndexedDB
  goes only with "all time"); cached files (always all). For one project or all. It closes
  that project's browser first (chats reopen it when they next browse) and is refused while a
  run is using it.
- Camera, microphone, location and notifications are always denied to pages.

## Debugging tools (Developer mode)

With **Settings › Browser › Developer mode › Enable full CDP access** on (the default), the
agent gets one `browser_devtools` tool for the chat's current tab, gated by the Debugging
permission (always allowed on local apps, Block on other sites by default). One tool with a
`panel` argument keeps the schema every request carries small:

| Panel | Does |
| --- | --- |
| `console` | Runs a JavaScript expression (awaited, top-level `await` allowed) in the page, or in the paused call frame, and returns its value or exception; without one, returns console messages at every level (log, info, warning, error, exceptions with stacks) newer than `since` |
| `elements` | Inspects one element (by CSS selector or snapshot ref): attributes, box, computed styles, HTML; or sets an attribute, style or text, or removes it, live (nothing is saved) |
| `network` | Lists the tab's requests (method, URL, status, type, size, time, failure); with an id, headers and response body; blocks URL patterns or throttles (offline, slow3g, fast3g) |
| `debugger` | Sets and removes breakpoints by script URL and line (optionally conditional), pauses on exceptions, pauses, resumes and steps; while paused, returns the call stack and the top frame's variables |
| `storage` | Reads, sets, removes or clears the page's cookies, localStorage or sessionStorage |
| `performance` | `metrics`: load timing, largest contentful paint, layout shift, resource sizes and page metrics; `trace`: records the main thread for 1–15 s, returns long tasks and scripting, rendering and painting time, and saves the trace as a file |
| `memory` | `summary`: heap, DOM nodes, listeners and documents, with growth since the last summary (to find leaks); `snapshot`: a heap snapshot with the largest object kinds, saved as a file |

Limits and safety:

- **Secrets hidden (best-effort):** cookie values, credential headers (`Authorization`,
  `Cookie`, `Set-Cookie`, `X-…-Token`) and token-like values (JWTs, long base64/hex strings,
  values under keys such as `password` or `token`) become `[hidden: N chars]` in results and
  chat history unless **Show secrets to the agent** is on. httpOnly cookie values are never
  returned. This guards against accidental exposure, not a page that talked the agent into
  encoding a secret; keep Debugging blocked on sites with accounts you care about.
- **Bounds:** the last 500 console messages and 300 requests per tab, at most 50 per call,
  16 KiB per value or body; traces and heap snapshots run one at a time.
- **Pauses:** a pause the agent leaves alone resumes after 30 seconds; when a run ends or is
  stopped, breakpoints, request blocks and throttling are cleared.
- Diagnostics record which tool ran and the permission decision, never expressions, values,
  bodies or sites.

