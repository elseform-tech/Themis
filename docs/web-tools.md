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

Asks your own [SearXNG](https://docs.searxng.org/) instance, a metasearch engine
that queries Google, Bing, Brave, DuckDuckGo and others. Set it in the user
`runtime.jsonc` (a project cannot):

```jsonc
{ "web": { "search_url": "http://127.0.0.1:8888" } }
```

SearXNG must allow JSON output: add `json` to `search.formats` in its
`settings.yml`. A local instance, for example:

```sh
mkdir -p searxng && printf 'use_default_settings: true\nserver:\n  secret_key: "%s"\n  limiter: false\nsearch:\n  formats: [html, json]\n' "$(openssl rand -hex 32)" > searxng/settings.yml
docker run -d --name searxng -p 127.0.0.1:8888:8080 -v "$PWD/searxng:/etc/searxng" searxng/searxng
```

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
no MCP server or Python to install. It uses Chrome, Chromium, Edge or Brave from
the usual install folders, or `browser.executable` from the user `runtime.jsonc`.
The browser runs headless with its own profile in app data
(`browser/profiles/<project>/`): logins persist across a project's chats and never
touch your own browser profile, Keychain or saved passwords. One browser serves a
project; each chat has its own tabs (at most 8). Downloads are blocked. Discarding
a chat closes its tabs; the browser exits when no chat of the project uses it.
It never outlives Themis: on macOS and Linux a small guard process ends the
browser and its helpers within seconds if Themis crashes or is force-quit.

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
for half a second; at most 30 s after a navigation and 10 s otherwise) and returns a
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

**`/browser-use`.** A built-in command (like `/manage-plugins`) that adds browser
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
OpenH264 encoder built into the app (no ffmpeg). One recording runs per chat at a
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

## Approvals

Each browser action asks for the site of the current page (for `browser_open`, the
site it opens); snapshots, scrolling and listing tabs are reads. **Allow in this
chat** on one browser action covers every browser action on that site, and a rule
like `{ "tool": "browser_*", "command": ["example.com"], "action": "deny" }` keeps
the agent off a site. The web tools ask per site, after they know it (the URL's host, or your SearXNG host
for searches). In Custom, rules may name a site and **Allow in this chat** covers
that tool on that site; see [composer permissions](configuration.md#composer-permissions).

```jsonc
{ "approval": { "rules": [
  { "tool": "fetch_url", "command": ["docs.rs"], "action": "allow" },
  { "tool": "fetch_url", "command": ["internal.example.com"], "action": "deny" }
] } }
```

YOLO fetches without asking; the OS sandbox does not apply to these in-process
requests, exactly as for MCP HTTP connections. Diagnostics never record page text
or search results.
