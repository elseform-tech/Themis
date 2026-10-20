# Web tools

Agents can read the web with two built-in tools. Nothing needs installing in
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

## Approvals

Both tools ask per site, after they know it (the URL's host, or your SearXNG host
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
