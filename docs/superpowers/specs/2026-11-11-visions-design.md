# Visions: agents that run your Themis chats for you

Date: 2026-11-11 · Branch: `codex/visions-design` · Design only. Nothing is built until the owner
says yes.

Design canvas: https://claude.ai/artifact/JZgtvsAnv7iiDQUmE7zjqq (version 47 and later). The
boards named below are on it.

## Why

Today the person manages every chat themselves. They start chats, re-prompt them, answer their
questions, check whether "done" is really done, and keep track of tabs. Steering goal runs,
distrusting reports and recovering stopped turns are all that management, done by hand.

Visions takes that job over. A **Vision** is an agent that sits on top of Themis. It does not
edit the project itself. It starts ordinary Themis chats, prompts and steers them, answers their
questions when allowed, checks their results, and reports to the person. Visions work in
**channels**: one channel is one department of one project (`# release`, `# watch`). The person
talks to the Visions in the channel instead of to each chat.

This is multiplayer between one person and their agents. There are no other people, accounts or
servers. Visions is meant to become the main way Themis is used, and plain chat stays as it is.

## Decisions already made by the owner

| Topic | Decision |
|---|---|
| People | One person plus Visions. No other humans (Mara on early boards was decoration). |
| Entry | The left rail is Chats, Visions (cloud), Integrations, Automations, then Settings, Help, Search. Chats is the default and always returns to plain chat. The sidebar toggle lives in the window's top bar, not the rail. |
| Sidebar | In Visions mode the channels bar replaces the chat list: project switcher, `# hq`, the project's channels, the Visions. |
| Vision chats | (Changed 2026-11-12.) Chats a Vision runs also appear in the plain chat sidebar, marked with the Vision's cloud. Typing in one, from either place, takes it over. Every control that hands a chat back to a Vision uses the cloud icon. |
| Creating | (2026-11-12.) The person creates channels and Visions by talking to the admin Vision in `# hq`, drawn with a crown. It proposes a setup and creates it only after the person says yes. The sidebar's + buttons next to Channels and Visions open the manual builders, for people who want to set things up themselves. |
| Voice | (2026-11-12.) Visions speak plainly, for people who don't code: no jargon, no command names, no file paths unless asked. |
| Transitions | (2026-11-12.) Before a Vision's post appears, the channel shows "<Name> is thinking…" with its cloud in the Thinking mood; posts fade in. Reduced motion turns both off. |
| Hint text | (2026-11-12.) No explanatory sub-lines or captions in Visions, and the rest of the app is swept for hint text that adds nothing, in its own PR. |
| Characters | Visions are the cloud cast. Bubbles is retired. |
| Questions | A Vision may answer its chats' questions, as far as its autonomy level allows (below). |
| Autonomy | A five-stop slider in Settings › Visions: Ask me first, Small calls, Most calls, Nearly all, Custom. The presets show what they allow, read-only; Custom makes each row editable. "Anything that leaves this computer" and "Spending past the budget" are always the person's, Custom included. Each Vision's profile can override the overall level. |
| Routines, heartbeat | Visions-only. Kept out of Automations, which stay for plain chats. |
| Quiet hours | Set once in Settings › Visions (no alerts, no new heartbeat work), and each Vision can override. |
| Status | The cloud has six moods, each tied to a real state: Calm, Thinking, Working, Needs you, Sorry, Asleep. Every mood also has a text label. The rail cloud shows the most urgent one, with a count badge for Needs you. |

## What a Vision is, in code

A Vision is **an agent run with a different job and a different credential**. It is not a new kind
of model loop. This reuses the run, tool, memory, usage and approval machinery that plain chats
already have.

- **Its own chat.** Each Vision has a hidden "Vision chat" per channel it belongs to. Its runs read
  the channel (paged history, see `get_thread_history_page`), its memory, its soul (`soul.md`) and
  its heartbeat note (`heartbeat.md`). Whatever it says to the person is posted in the channel.
- **No project writes.** A Vision run has read-only access to the project. Building happens in
  the chats it starts, which run in their own worktrees with the channel's approval mode.
- **Driving chats through `themis`.** The owner chose the `themis` CLI over new agent tools
  (2026-11-07 spec). Today an agent's credential is bound to its own chat:
  `crates/desktop/src/server/agent.rs:304` refuses `send_message`, `stop_thread`,
  `answer_question` and `approve_action` for it. Visions adds a **Vision grant**:
  - It is issued to a Vision run and names the Vision and its channel.
  - It allows `create_thread` (in the channel's project only), plus `send_message`,
    `steer_queued_message`, `stop_thread`, `answer_question` and the existing chat reads, **only
    for chats this Vision owns**. Ownership is checked on the request (the `threadId`), never on
    command text, the same way `refusal` works today.
  - It **never** allows `approve_action`, `set_thread_approval_mode`, settings writes, secrets,
    switching on a hook or a command MCP server, or anything on chats it does not own. These stay
    on the existing refusal list, with tests.
- **Owned chats.** A chat gains an owner record (Vision id, channel id) in the Visions store.
  Because of that record the Vision's grant can reach it, and the plain sidebar marks it with the
  Vision's cloud. The person can always open, type in, stop or delete it, from the channel or from
  plain chat.

### Waking up

A Vision must not poll. It wakes, meaning a run starts in its Vision chat, on:

1. A message from the person in its channel that mentions it, or one routed to it by the channel's
   chief of staff.
2. An event from a chat it owns: the turn finished, the turn failed, the chat asked a question, an
   approval is waiting, or the person took over or handed back.
3. Its heartbeat or one of its routines coming due (phase 2).

Wakes are bounded like `hook_wakes.rs`: a cap on Vision runs in a row without a message from the
person, none after the person stops it, none while it is paused or out of budget, and none during
quiet hours for heartbeats. Events that arrive while a Vision runs are queued and read in its next
run, never dropped.

### The person stays in charge

- **Approvals.** An approval from a Vision's chat shows in the channel (Wren's push on the boards).
  Only the person's Allow or Deny answers it, through the existing `approve_action`. Auto and YOLO
  behave in a Vision's chats exactly as in any chat. A Vision never chooses or loosens a mode.
- **Stop is final.** Stop on a running line ends that chat's run. Pause on a Vision stops its
  chats, heartbeat and routines, and it starts nothing new. A Vision's next run is told what the
  person stopped and must not restart it until asked. A test covers this. It is the same failure
  as the stopped-turn context gap.
- **Taking over.** When the person types in a Vision's chat, from the channel or from plain chat,
  the Vision stands by. It reads what happened when the person presses "Hand back", a button with
  the cloud icon.
- **Autonomy.** When an owned chat asks a question, the Vision decides how much the choice
  matters. If its level allows that kind of decision, it answers with `answer_question` and posts
  the question, its answer and the reason in the channel, with "Change the answer". Otherwise it
  passes the question on, and the person answers in the channel. If the Vision is unsure, it
  passes the question on.
  - **Honest limit:** the impact judgement is the model's. The server cannot read a question and
    know its impact. The two locked rows are enforced by real gates, not by the model's judgement:
    sending work out of the computer needs an approval, and spending is stopped by the budget. The
    other rows are measured by the eval below, not enforced.
- **Budget.** Usage already records cost per request (`state/usage.rs`). A Vision's daily spend is
  its own runs plus its owned chats. At the cap, the server stops the runs (not the model), and
  the channel shows "reached today's budget" with Raise and Leave buttons. There is also an overall
  daily cap for all Visions.

### The admin Vision (`# hq`)

Each install has one built-in admin Vision, Atlas, drawn with a crown. It is the only member of
every project's `# hq`, can't be deleted, and builds nothing itself.

- **What it does.** The person tells it what they want handled. It proposes channels and Visions
  (name, what each Vision does, members, model) as a proposal card in `# hq`, and asks. When the
  person says yes in their own words, it creates them. It can also rename, change a Vision's
  role, add or remove members, and pause or resume, the same way: propose, then act on a yes.
- **Its grant.** The admin's runs get the Vision grant without chat-driving methods, plus
  `create_channel`, `create_vision`, `update_vision` and `update_channel` for its proposals. It can
  never raise autonomy above Settings › Visions, raise a budget, approve anything, delete a channel
  or a Vision, or touch settings, secrets, hooks or MCP servers.
- **The yes is checked by the server, as far as it can be.** Each create or change names a
  proposal the admin posted; the server refuses it unless that proposal is unexecuted, matches the
  request, and the person has posted in `# hq` after it. Whether the reply means yes is the model's
  judgement, measured by the eval, not enforced.
- **Voice.** It says what it will set up in plain words and keeps to one short message per turn.

### Checking work, not trusting it

A Vision reports "done" only with evidence it read itself: the chat's `files_changed`, the test
command's output, the goal check's verdict where there is one. It never relays the chat's own
summary as proof. The eval below measures this. A false "done" from a Vision is the main risk of
this whole feature, because it turns one false claim into two.

## Storage and configuration

- **Store:** app-data `visions/`. It is revision-checked like `plugins/registry.json` and written
  only through validated store APIs:
  - `visions.json`: profiles with name, role, cloud shape and colour, model, and overrides for
    autonomy, quiet hours and budget.
  - `channels.json`: project, topic, chief of staff and member Visions.
  - `owners.json`: which chat belongs to which Vision and channel.
  - Per-Vision `soul.md` and `heartbeat.md`.
  - The channel logs as bounded JSONL.
- **Settings › Visions:** overall autonomy, the Custom rows, daily budget, model for new Visions,
  quiet hours, notifications, and Pause all.
  - **Needs the owner's decision:** AGENTS.md allows in-app editing only for the `browser`, `web`,
    `subagents` and `memory` blocks.
  - **Recommendation:** a `visions` block in `runtime.jsonc` written field by field, with Settings
    › Visions added to that list in AGENTS.md and `docs/configuration.md`. This keeps provenance,
    validation, and the rule that a project may only add restrictions (a project could lower
    autonomy, never raise it). Profiles and channels stay in the app-data store because they are
    content, not configuration.
- **Diagnostics:** bounded metadata events (Vision id, channel id, chat id, wake reason,
  outcome), never message text, per the logging rules.

## Phases

**Phase 1: one working loop.** This phase must prove a Vision saves the person messages without
false "done" claims.
- Channels (one project each), Visions with profiles, and the Vision grant with ownership checks.
- Wakes on owned-chat events, Stop, Pause, taking over and handing back.
- Questions with the autonomy slider and Custom, and approvals relayed into the channel.
- Daily budgets enforced by the server.
- UI: the rail cloud with its six states, the channels bar, the channel view, thread links with
  Stop, the thinking row, Vision chats marked in plain chat with the cloud hand-back, Vision
  profile, Settings › Visions, first run in `# hq` with the admin creating channels and Visions,
  and problem states.
- Plain, non-technical voice in every Vision prompt.
- Bubbles is retired, in a small separate PR first.

**Phase 2: running on its own.** Heartbeat and routines with quiet hours (a Visions-only timer,
separate from Automations), the chief of staff routing work between Visions, with a cap on
handoffs so Visions cannot ping each other forever, the admin's own suggestions (proposing work
unasked), and roles.

**Phase 3: polish.** Notifications on every OS (today `notifications.rs` delivers on macOS only,
and the "ship on every OS" rule applies), plus search across Vision chats.

**Separate track:** the reskin (sky themes) ships on its own, Classic first, after a Linux
performance check of the blur.

## Proof

- **Server and CLI tests:**
  - The Vision grant allows only owned chats and refuses approvals, modes, settings, secrets and
    switching things on.
  - Wakes are bounded and none happen after Stop.
  - The budget stops runs.
  - Locked rows hold under Custom.
- **Eval `evals/visions`:** a scenario with hidden checks, run on cheap models within the eval spend
  policy:
  - Measures:
    - How many messages the person needs to get the work done, compared with driving the same task
      in goal mode by hand.
    - False "done" claims, checked against the real files and tests.
    - Questions answered above the autonomy level.
    - Restarts after a stop.
  - A labelled set of chat questions measures the impact judgement on its own.
  - Phase 1 is done when a Vision needs fewer person messages than goal mode with no false "done"
    claims across the agreed number of runs.
- **Live check:** rows in `docs/live-verification.md` for the Visions rail entry, a channel, a
  Vision profile and Settings › Visions, plus a `feature:` row for taking over, so the
  `live_coverage` test covers them.
- **Scale:** channel history, wake queues and the owner lookup grow with long use, so they get a
  scale test that counts work at two sizes (`docs/scale-tests.md`).

## Naming

`crates/core/src/vision.rs` already means "screenshots for models that read images". The new
code uses `visions` (plural) everywhere: module `visions`, store `visions/`, config `visions`. The
old module is left alone.

## Settled on 2026-11-11

1. **Settings › Visions** is a `visions` block in `runtime.jsonc`, written field by field from the
   app. AGENTS.md and `docs/configuration.md` add it to the blocks the app may edit in place.
   Profiles, channels and ownership stay in the app-data `visions/` store.
2. **Default autonomy** is Small calls.
3. **Approval mode:** each channel has one approval mode, set by the person in channel details. A
   Vision's chats use it and a Vision cannot change it. It starts as the person's default mode.
4. **Budget:** there is a default daily cap for all Visions together, shown in Settings › Visions
   and raisable there. It is never "no cap" by default. The starting value is set in the
   implementation plan from measured phase 1 eval costs, so it is not guessed here.
