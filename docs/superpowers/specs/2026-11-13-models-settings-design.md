# Models settings: one place for every model choice, with fallback

Owner-approved board: Settings › Models on the design canvas
(https://claude.ai/artifact/JZgtvsAnv7iiDQUmE7zjqq, board "Settings › Models", version 68).

## Why

On 2026-11-13 the live check showed what scattered model settings cost. The Auto reviewer ran on
`muse-spark-1.3-contributor`, the monthly OpenCode Go allowance was used up, and every review
failed, so every write in a work chat became a question for the person. Nothing on screen said
which model the reviewer was, that it was failing, or how to change it: `approval.auto_model` is
file only. The same day `longcat-2.5-free` hit a 13-hour rate limit and `iceman-contributor-free`
blocked Vision runs with a content filter; each failure stopped the work instead of moving on.

Model choices live in four places today: Settings › Models (the chat default and
`memory.models`), Settings › Subagents (`subagents.model`, `subagents.models`),
`approval.auto_model` (file only) and `compaction.summary_model` (file only), plus a Vision's
profile model. People who want different models for different jobs have no single place to see or
set them, and only memory jobs fall back to another model.

## Goals

- One screen, Settings › Models, that shows every model choice and the provider's state.
- Each job Themis runs by itself has an ordered list: the first model is used, the next ones only
  when the one before can't answer, so a rate limit or a used-up allowance no longer stops work.
- The person sees why a model or provider can't answer (rate limited until when, allowance used up
  until when) in Settings, and nowhere it would be noise.
- The chat's model picker searches every model of every connected provider, filtered by provider.
- Evals measure exactly the model they name: fallback off.

## Scope and order (owner, 2026-11-13)

Built on `main` now, without the Visions row: Visions is not on `main` yet (it sits on the unmerged
`codex/visions-*` chain). When that chain lands, it adds the Visions row and a `visions` list, and
settles how a Vision's own runs fall back, where those runs are defined. Everything below marked
"(Visions chain)" is for that work, not this one.

## Non-goals

- No change to which providers exist or how keys are stored (Settings › Secrets, `providers`).
- No automatic choice of "the best model" for a job. The person's lists decide.
- No fallback for the chat. The person picked that model.
- No per-model cost caps beyond what exists (`memory.daily_budget_usd`, Visions budgets).

## Anti-goals (a failure even if it works)

- A silent model switch nobody can find out about afterwards.
- Fallback hiding a Themis bug: a request Themis got wrong (an ordinary HTTP 400) must fail as
  today, not move to another model.
- A settings page that grows with the catalog. The page lists jobs, not models.
- Status text in places where it is noise (pickers, the Themis-picks lists).

## What the person sees (as approved on the board)

### Settings › Models

1. **Providers**, one card each: the provider's mark (OpenCode Go's logo; the board's "OC" is a
   placeholder) and name, and its state: connected, or a provider-wide condition such as
   "Allowance used up · back 21 Nov". The allowance is the account's quota for the provider's paid
   models, never shown per model. Free models keep answering while it is used up (checked
   2026-11-13 13:42: `beam-free` and `iceman-contributor-free` answered while every paid Go model
   returned `GoUsageLimitError`).
2. **You pick**: Chat only, one default model for new chats, chosen with the model picker.
3. **Themis picks**: Subagents, Visions (added by the Visions chain), Auto reviewer, Summaries, Memory. Each row shows its
   ordered list as comma-separated names; opening a row shows the list with drag handles, position
   numbers, the model name, its provider label, a remove button and **Add**. No status in these
   lists.
4. **Fall back to the next model**, one switch, default on, for all Themis-picks jobs.

**Add** opens the model picker below it. Models already in the list are greyed with a check;
choosing another appends it to the end.

### The model picker (chat header, Settings › Models › Chat, Add)

A popover with a search field, a row of provider chips ("All", then one chip per connected
provider with its mark), and every model of the connected providers in alphabetical order, each
with its provider's mark and the name, the current one checked. No status, no "Default" label.
Search matches model and provider names. It scrolls; the full catalog lives only here.

### In a chat

The header shows the chat's model name only. A model that can't answer fails the reply as today,
with the reason in the failure ("beam-free is rate limited until 01:00"); no "Try with" offer.

## Behaviour

### Jobs and lists

| Job | Setting | Used for |
|---|---|---|
| Chat | one model | new chats' starting model; the person changes it per chat |
| Subagents | ordered list | a subagent's model: the agent may choose any model in the list for one subagent; otherwise the first. The chat's model stays the last thing a subagent tries, as today |
| Visions (Visions chain) | ordered list | a new Vision's model is the first; a Vision's own model (its profile) is tried first for its runs, then this list |
| Auto reviewer | ordered list | Auto mode reviews |
| Summaries | ordered list, or automatic | compaction and document summaries; unset keeps today's automatic summary routing |
| Memory | ordered list | memory jobs |

A list holds 1–8 distinct entries. An entry is a model id or `"chat"`, meaning the chat's model
(the picker shows it as "Same as chat" at the top). An empty list is not allowed; a job never
configured has the default below.

### Fallback

Applies to Themis-picks jobs only, when `fallback` is on. A request moves to the next model in the
job's list only when the model **can't serve**:

- HTTP 429 (rate limit) or the provider's quota or allowance error (OpenCode Go's
  `GoUsageLimitError`);
- a provider content-safety block (`content_safety_violation`, also the streamed form OpenCode Go
  reports as HTTP 400 `server_error` "Upstream response was not valid JSON" once the existing
  retries are spent);
- a server error (5xx, 499, Go's `server_error` 400) still failing after the existing retries;
- the model is not in the captured catalog, or the provider refuses it (402, 404, 410).

It never moves on an ordinary 400 or other request error. A run that moved stays on the new model
for the rest of that run. When every model in the list fails, the job fails as it does today, with
the last reason.

Each move writes one bounded diagnostics event (job, from-model, to-model, reason class; ids only).
Where a person reads the result, it says which model answered: a subagent's card names the model it
ran on, a Vision's channel message is unchanged (the Vision's chat records the model), the reviewer
and summaries stay internal.

### Model health

Themis keeps a small in-memory record, learned from responses only (no polling): per model, rate
limited until (from `Retry-After`, capped) or refused; per provider, its paid models' allowance used
up until (from the provider's message, else 24 hours). The allowance mark skips only that
provider's paid models (the catalog's price says which); free models are still tried. Fallback skips
a model whose record says it can't serve yet, so a 13-hour rate limit costs one failed request, not
one per job. Only the provider-wide state is shown, on the provider card. The record resets on
restart.

### Defaults (day one behaves as today)

- Chat: stays the existing app setting (`default_provider`, `default_model` in the app's
  settings), which Settings › Models already writes. It is not in `runtime.jsonc`.
- Subagents: `["chat"]`, today's "the chat's model".
- Visions (Visions chain): `["chat"]`, today's "the app's default model" for a Vision without its own.
- Auto reviewer: today's rule, the first memory model, so the memory default order.
- Summaries: unset, so today's automatic routing (including `longcat-2.5-preview-free` with
  thinking off, and the retry on the chat's model after 402/404/410). Setting a list replaces that
  routing with the list.
- Memory: today's `memory.models` default order.

## Configuration

One `model_jobs` block in the person's own `runtime.jsonc`, written field by field from Settings ›
Models (an AGENTS.md exception like `browser`, `web`, `subagents`, `memory`). The existing `models`
map (per-model overrides such as `context_tokens`, which a project may set) is unchanged.

```jsonc
"model_jobs": {
  "jobs": {
    "subagents":     ["iceman-contributor-free", "beam-free"],
    "auto_reviewer": ["beam-free", "iceman-contributor-free"],
    "summaries":     ["iceman-contributor-free", "beam-free"],
    "memory":        ["iceman-contributor-free", "beam-free"]
  },
  "fallback": true
}
```

- Model ids as today (`<provider>/<id>` for a model from `providers`), validated against the
  captured catalogs at run start.
- It **replaces** `subagents.model`, `subagents.models`, `memory.models`, `approval.auto_model` and
  `compaction.summary_model` outright: those fields are removed, not aliased (the owner's
  no-backward-compatibility rule). A file that still has them fails validation with a message
  naming the new field.
- Layers: `model_jobs` is user file only, except `fallback`, which a run override may set (evals).
  A project layer setting any of it is refused, as `approval.auto_model` is today.
- Captured once per run with the rest of the configuration.

## Evals

Every eval harness pins its models and sets `model_jobs.fallback: false` through the run override, so a
result always names the model that produced it. The eval records note it.

## Visibility and diagnostics

- Provider state on the provider card; nothing else shows status.
- Diagnostics: `model_fallback` (job, from, to, reason class), `model_health` (model or provider,
  state, until), both metadata only.
- A live-check row (`feature:models-settings`) and the configuration, user and integration guides
  updated with the new block and the removed fields.

## Decided by the owner

- One Models page: providers on top, then You pick (Chat) and Themis picks (Subagents, Visions,
  Auto reviewer, Summaries, Memory). No global ordered model list, no allowed list, no tabs.
- Chat: a default only, no fallback, no "Try with" offer.
- Subagents and Visions are Themis-picks jobs with ordered lists and fallback.
- Status: on the provider card only; not in pickers or job lists. Allowance is per provider.
- Picker: search plus provider chips, one column, provider mark on each row, all models.
- Fallback switch: one, for Themis-picks jobs; off for evals.

## Calls made while writing this (check these)

- The new block is `model_jobs`, since `models` already holds per-model overrides.
- The chat default stays an app setting, not `runtime.jsonc`.
- A list entry `"chat"` ("Same as chat") keeps day-one behaviour for Subagents and Visions; a
  subagent still tries the chat's model last.
- Summaries keep today's automatic routing until a list is set.
- The allowance mark applies to a provider's paid models only.
- Model health lives in memory and resets on restart.

## Left to the builder

- Exact error-class mapping per wire protocol, as long as it follows the "can't serve" rule above.
- The health record's caps and how a quota message's reset time is parsed.
- Where the picker component lives and how it is shared between the three places.
- How the subagent card shows the model it ran on, within DESIGN.md's rules.
