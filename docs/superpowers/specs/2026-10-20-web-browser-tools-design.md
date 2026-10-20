# Web search, fetch and in-app browser

Status: approved 2026-10-20 (design canvas "Themis In-App Browser"). Branch `codex/web-browser-tools`.

## Goal

Give agents the open web without installing anything: `web_search` and `fetch_url`
for reading, and a built-in Chromium the agent drives and the person can watch,
take over and inspect with DevTools inside Workspace. Screenshots and recordings
can be shown inline in replies.

## Decisions

- **Search backend: SearXNG**, self-hosted by the person, JSON API. Measured
  2026-10-20 (local Docker, one home IP): results are live, not a snapshot, but the
  engine mix flaps (Brave/Google CSE served; Google, Qwant, Bing, DuckDuckGo
  suspended or CAPTCHA within a few dozen queries; the news category returned
  nothing). The tool therefore reports failed engines, counts the results array
  (never `number_of_results`), searches the general category and paces queries.
  The backend sits behind a small trait so a paid API can be added later.
- **No MCP, no Python.** Browser control is a native CDP client in `crates/core`
  over `tokio-tungstenite`. It talks to the person's Chrome, Chromium or Edge, or to
  a Chrome for Testing copy downloaded into app-data on request.
- **One Chromium per chat, headless, own profile per project** (logins survive
  between chats of a project; the person's real browser profile is never used).
- **Same browser for agent and person.** The Workspace Browser tab shows CDP
  screencast frames and forwards the person's input. "Take over" pauses agent
  browser actions ("the user is controlling the browser") until "Hand back".
- **Workspace rule exception.** Workspace stays read-only for project state. The
  Browser tab is the second interactive exception (after Tasks): navigation,
  input, take over, capture, record, DevTools. None of it changes project files.
- **Approvals are per site.** `fetch_url`, `web_search` and the browser tools
  approve themselves inside `execute` (the host is known only there), with the
  host as the action's `command`. Rules may name a site:
  `{"tool":"browser_*","command":["example.com"],"action":"deny"}` (subdomains
  match). "Allow in this chat" covers that site: every browser action on it, or
  `fetch_url` on it. Snapshots and screenshots of the current page are `Read`.
- **Untrusted content.** Every page text, snapshot, search result and console line
  is labelled untrusted data and never grants permission. Password fields are
  always masked in snapshots. Diagnostics never record URLs' query strings, page
  content or input text.

## Tools

| Tool | Arguments | Risk |
| --- | --- | --- |
| `web_search` | `query`, `time_range?` (day/week/month/year), `max_results?` (10, ≤20) | Network, site = SearXNG host |
| `fetch_url` | `url`, `offset?` | Network, site = URL host |
| `browser_open` | `url`, `new_tab?` | Network, site = target host |
| `browser_navigate` | `action`: back/forward/reload | Network |
| `browser_snapshot` | `ref?` | Read |
| `browser_click` | `ref`, `double?` | Network |
| `browser_type` | `ref`, `text`, `submit?`, `clear?` | Network |
| `browser_select` | `ref`, `values` | Network |
| `browser_press` | `key` | Network |
| `browser_scroll` | `direction`, `ref?` | Read |
| `browser_tabs` | `action`: list/switch/close, `tab?` | Read (list/switch), Network (close) |
| `browser_screenshot` | `ref?`, `full_page?` | Read |
| `browser_record` | `action`: start/stop, `max_seconds?` (30, ≤120) | Read |
| `attach_file` | `files: [{path, caption?}]` (≤12 per reply) | Read |

Every browser action waits for the page to settle (bounded) and returns a fresh
snapshot: a compact accessibility outline with stable refs (`[e12] button "Sign
in"`), capped in size, plus what is new since the last action (console errors,
dialogs, failed requests, downloads).

## Limits

- `fetch_url`: http/https only, no credentials in URLs, 5 redirects, 5 MiB body,
  30 s, 8,000 characters per page with `next_offset`; HTML is reduced to its main
  content as Markdown, text/JSON/XML pass through, other types are refused.
- `web_search`: at most 30 searches per run, at least 1 s apart.
- Browser: one Chromium per chat, 8 tabs, snapshot ≤ 12,000 characters, action
  settle wait ≤ 10 s, navigation ≤ 30 s.
- Media: latest 2 screenshots go to vision models as images, older ones become a
  text pointer; one recording per chat at a time; 200 MB / 500 files per chat, oldest
  unattached captures removed first.

## Configuration (`runtime.jsonc`)

```jsonc
"web": { "search_url": "http://127.0.0.1:8888" },   // user file only; unset hides web_search
"browser": { "enabled": true, "executable": "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" }
```

A project may only set `browser.enabled: false` and may not set `web.search_url`
or `browser.executable`.

## Build order

1. `fetch_url`, `web_search`, configuration, per-site approvals; fuzz + property tests.
2. CDP client, browser launch, snapshot and action tools.
3. Desktop browser session, screencast and input, take over, DevTools, Workspace tab.
4. Media: screenshot files, Capture, recording, `attach_file`, inline media + viewer.
5. Image input for vision-capable models.

Each step lands with shared-server/CLI regression tests and docs; the finished
branch is verified end to end with a real model and the real app.
