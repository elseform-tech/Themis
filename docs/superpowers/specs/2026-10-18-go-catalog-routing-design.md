# OpenCode Go: catalog-driven routing, Gemini, and project row actions

Date: 2026-10-18 · Branch: `codex/go-catalog-routing` · One PR, separate commits per part.

## Why

OpenCode Go keeps adding models. The model list and limits already update live
(Go `/models` + models.dev), but *how* each model is called is guessed from its
name (`providers/wire.rs` `go_chat_path`: `muse-`/`gpt-`/`grok-` → Responses,
`qwen`/`minimax-`/`claude-` → Messages, else Chat Completions). A new family
silently gets the wrong protocol. Go has now added `gemini-4-argon-contributor`,
which needs a fourth protocol (Google Gemini) we do not speak.

The person wants: new Go models picked up without app releases, every Go model
usable, and the code organized so a new protocol is one self-contained addition.

Separately, project Rename/Delete only act on the *active* project and live as
menu entries, so acting on another project means switching to it first.

## Goals

1. Each Go model's protocol and endpoint come from the models.dev catalog, not
   from its name.
2. Add the Google Gemini protocol, so every model on the Go docs' endpoint list
   is callable, including tools, streaming, and thinking/reasoning replay.
3. Organize providers so each protocol is its own module behind one shared
   interface, and all per-model facts sit in one catalog record.
4. Project dropdown rows show Rename, Delete, and the chat count side by side,
   always visible (no hover reveal), for any project.
5. Verify end to end: every Go model answers a tool-using request through the
   real app server/CLI, and the desktop app is driven with computer use.

## Non-goals

- No JavaScript/Node sidecar or Vercel AI SDK; the agent stays in Rust.
- No new providers beyond OpenCode Go.
- No in-app catalog editor, no manual "refresh models" control beyond what exists.
- No undo-toast delete: the "Are you sure?" dialog stays.

## Anti-goals (failure even if it "works")

- A model is silently sent the wrong protocol. Unknown protocol must show as
  unavailable with a reason, never a guessed call.
- Logging prompts, provider bodies, or the API key while adding Gemini.
- Losing the per-chat `x-opencode-session` header or the Themis user agent on any
  protocol.
- Regressing existing protocols (reasoning replay, retries, output limits).

## Facts gathered (2026-10-18)

- Go docs (`opencode.ai/docs/go/`) list endpoints under `https://opencode.ai/inference/go/`:
  - `openai/v1/responses` — Responses (`@ai-sdk/openai`): grok, gpt-luna, muse.
  - `anthropic/v1/messages` — Messages (`@ai-sdk/anthropic`): claude, minimax, qwen.
  - `google/v1beta/models/<id>` — Gemini (`@ai-sdk/google`): gemini-4-argon-contributor.
  - `openai/v1/chat/completions` — Chat Completions (`@ai-sdk/openai-compatible`): everything else.
  - `v1/models` — model list.
- models.dev `api.json` → `opencode-go.models.<id>.provider` gives `npm` and `api`
  per model (absent `npm` means the provider default `@ai-sdk/openai-compatible`;
  provider-level `api` is `https://opencode.ai/inference/go/openai/v1`).
- Go `/v1/models` returns IDs only (no protocol metadata). Both the current
  `https://opencode.ai/zen/go/v1` and `/inference/go/v1` answer `/models` today;
  docs only mention `/inference/go/`.
- Docs name no auth header. `@ai-sdk/google` sends `x-goog-api-key`; the others
  use `Authorization: Bearer` / `x-api-key` as today.

## Design (decided)

### Model record

`GoModel` (serialized to the UI) gains:

- `protocol`: `chat_completions | responses | messages | gemini | unsupported`
- `endpoint`: the per-model base URL from the catalog (`provider.api`, else the
  provider-level `api`).
- `unavailable_reason: Option<String>` when `protocol` is `unsupported`
  (e.g. "Needs @ai-sdk/xyz, which Themis does not support yet").

Mapping from catalog `npm`: `@ai-sdk/openai` → responses, `@ai-sdk/anthropic` →
messages, `@ai-sdk/google` → gemini, `@ai-sdk/openai-compatible` or absent →
chat_completions, anything else → unsupported.

### Fallback when models.dev is unreachable

Keep today's name-prefix table (plus `gemini-` → gemini) as the fallback only,
with endpoints derived from the protocol. A model matching no prefix falls back
to chat_completions, as today, because the Go docs say that is the default
endpoint. The UI never blocks on models.dev.

### Base URL and test override

Default endpoints move to `https://opencode.ai/inference/go/...` per the docs.
The existing base-URL override (tests, `go_base_url_override`, CLI) still wins:
when set, every protocol is served from that one root
(`{override}/responses`, `{override}/messages`, `{override}/chat/completions`,
`{override}/models/{id}:streamGenerateContent`), so wiremock tests keep one server.

### Routing seam

Routing no longer reads the model name. The resolved route (protocol + endpoint)
travels with the catalog the same way limits and thinking already do
(`with_catalog_limits` task-locals, or an explicit field on `ProviderConfig` —
builder's call), and the provider asks the route, not `go_chat_path`.

### Module layout

Split `crates/core/src/providers.rs` (3,586 lines) and `providers/wire.rs`:

- `providers/catalog.rs` — models.dev + `/models` fetch, `GoModel`, protocol
  mapping, fallback table, task-local lookups (`thinking_default`,
  `bounded_output_limit`, etc.).
- `providers/protocol/{chat_completions,responses,messages,gemini}.rs` — each
  builds the request body, request path/headers, and parses stream + non-stream
  responses into the shared internal shape. One shared trait/enum dispatch.
- `providers/reasoning.rs` — stays; gains Gemini's thought-signature handling.
- `providers.rs` — `ProviderKind`, `ProviderConfig`, `resolve`, the
  `CompatibleProvider` transport (HTTP, retries, error mapping, headers).

Pure moves first (no behavior change, tests green), then the routing change,
then Gemini, as separate commits.

### Gemini protocol

- Request: `POST {endpoint}/models/{id}:streamGenerateContent?alt=sse`
  (non-stream: `:generateContent`), headers `x-goog-api-key`, `x-opencode-session`,
  Themis user agent. Confirm the auth header live; if Go wants Bearer, use that.
- Body: `contents` (roles `user`/`model`), `systemInstruction`,
  `tools[].functionDeclarations`, `generationConfig` (`maxOutputTokens`, thinking
  config from the catalog's reasoning options).
- Tool calls: `functionCall {name, args}` parts; results go back as
  `functionResponse {name, response}` parts in a `user` turn. Gemini may omit call
  IDs; synthesize stable IDs and map results back in order.
- Tool schemas: strip JSON-Schema keywords Gemini rejects (e.g. `$schema`,
  `additionalProperties` where unsupported) without changing tool behavior.
- Reasoning replay: keep each part's `thoughtSignature` and send it back on the
  same `functionCall` part, consistent with `reasoning.rs` for other protocols.
- Errors map to the existing typed `LLMError`s (auth / rate limit / generic).

### Other hardcoded names

- `summary_model` (`providers.rs:158`): keep the configured override; replace the
  hardcoded name list with "same model if it is in the catalog and usable, else
  the configured fallback". Do not hardcode `longcat-2.5-preview-free` beyond the
  existing default constant.
- Claude-specific thinking (`starts_with("claude-")`): key on protocol
  `messages` + catalog reasoning options instead, if behavior is identical for
  qwen/minimax; otherwise keep it and leave a comment why.
- Default model `muse-spark-1.3-contributor` is duplicated in three web files;
  make it one exported constant. If the saved default model is no longer in the
  live catalog, the composer picks the first usable model rather than failing.

### Picker UI

Unsupported models show greyed in the model picker with the reason as a
tooltip/inline note; they cannot be selected. No new controls.

### Project row actions (desktop UI)

In the project dropdown (`web/src/screens/Sidebar.tsx`), each project row shows,
right-aligned and always visible: Rename (pencil), Delete (trash), chat count.
Icons act on that row's project, not the active one. The built-in Themis project
has no Delete icon (keep alignment). Remove the old "Rename project" and
"Delete project" menu entries; keep "New project" at the bottom. Delete opens the
existing "Delete project?" dialog unchanged. Icons need accessible labels
(`Rename <name>`, `Delete <name>`), Lucide paths like the rest of the sidebar,
and must not shrink the name to nothing at the 240px minimum width. Follow
DESIGN.md and docs/ui-rules.md.

## Verification required

- Unit/integration: catalog mapping (each npm value, missing, unknown), fallback
  table, override routing, Gemini body/stream/tool-call/thought-signature parsing
  (wiremock), schema sanitizing. The catalog and Gemini stream parsers take
  untrusted provider JSON → property tests and a fuzz target per AGENTS.md.
- CLI end-to-end via the local app server: one Gemini tool round trip (mock), and
  an unsupported-model rejection.
- Live sweep (benchmark audit, isolated HOME/data dir, key from `OPENCODE_KEY`):
  every model in the live Go catalog completes a small tool-using request;
  report per-model pass/fail.
- Web: Vitest for row icons acting on non-active projects, no Delete on Themis,
  dialog still confirms; unsupported model not selectable.
- Desktop by computer use: rename and delete a non-active project from the
  dropdown; pick Gemini and complete a chat with a tool call.
- Full check list from AGENTS.md, quality dashboard refresh, docs updates
  (configuration.md / user-guide.md: endpoints, protocol source, unsupported
  models).
- Then: PR, self-review, CI green.

## Calls made by Claude (not yet confirmed by the person)

- Default endpoints move from `/zen/go/v1` to `/inference/go/...` (docs-aligned;
  both answer today).
- Unknown-prefix fallback stays Chat Completions when models.dev is down.
- Rename keeps the existing rename dialog (not inline editing).
- Themis row keeps an empty slot where Delete would be, so counts line up.
- Live-sweep failures caused by quota/rate limits or account access are reported
  as such, not treated as app bugs.

## Left to the builder

- Exact trait vs. enum dispatch for protocols; where the route is carried.
- Gemini thinking-config field names for this model (read from the catalog's
  reasoning options and confirm live).
- Icon sizing/spacing within DESIGN.md tokens.
