# Visions teams: one chief of staff per channel

Date: 2026-11-13 · Branch: `codex/visions-design` · Design only. Builds on the Visions design
(`2026-11-11-visions-design.md`) and its Phase 1 code; replaces parts of it where noted.

Order (owner, 2026-11-13): Phase 1 is finished and merged first; this is built on top as Phase 2,
with the one-time conversion of existing channels described below.

Design canvas: https://claude.ai/artifact/JZgtvsAnv7iiDQUmE7zjqq (version 81 and later), boards
"Visions redesign: a team channel", "the chief's expressions, then the whole cast in 3D",
"# administration, Atlas builds a team and adds a worker" and "the chief of staff's profile".

## Why

Phase 1 puts several Visions in a channel and the person talks to whichever one they @mention.
That leaves the person managing agents instead of chats, and a post that mentions nobody wakes
nobody. The owner's picture is a company where teamwork matters: each channel is a department
with one manager the person talks to, workers who do the work, and departments that help each
other. The person deals with one contact per channel and never has to coordinate agents.

Success: the person writes to one chief per channel and the work gets done, with fewer messages
than Phase 1 and no false "done" claims. Anti-goals: agents chattering at the person, agents
bouncing work between each other while spending money, and any agent gaining powers the person
didn't give it.

## Decisions made by the owner (2026-11-13)

| Topic | Decision |
|---|---|
| Shape | Each channel has one **chief of staff** and one or more **workers**. Atlas runs `# administration`. |
| Contact | The person talks only to the chief. Workers never talk to the person. |
| Orders | Only the chief gives workers tasks. Workers think for themselves, report back to the chief and may push back; the chief decides. |
| Chief's work | The chief reads, answers, plans, delegates, and pauses or redirects its workers. It never changes anything itself; every change is a worker's. (Recommended by Claude, agreed.) |
| Visibility | The channel shows only the person and the chief. Worker–chief exchanges stay out of it but are one tap away, read-only, through the worker chips. When pushback matters, the chief tells the person in its own words. |
| Hiring | A chief that lacks a skill asks Atlas; Atlas proposes the worker in `# administration` and adds it only on the person's yes. Only Atlas creates channels, chiefs and workers. |
| Between channels | Chiefs send each other requests, never orders. The receiving chief accepts or declines within its autonomy level, without the person's yes. Each side shows one line. Work is paid from the budget of the channel that does it. A daily cap stops loops. |
| Souls | Only chiefs and Atlas have a soul. It is a `soul.md` file per agent, editable in the profile and openable in the person's editor for advanced use. Workers have a role, not a soul. |
| Routines | Routines belong to the chief. The chief may add one within its autonomy level and says so in the channel; the person can pause or remove any routine. |
| `# administration` | Replaces `# hq`. It is a channel pinned at the top of the Channels list, with Atlas as its chief and no workers. |
| Look | The channel header shows the chief large and in 3D with live expressions, next to a decorative channel name and the worker chips. Every character keeps its own colour, body shape and accessory (Juno's clipboard, Atlas's crown, Rook's hard hat, Wren's glasses, Pip's goggles, Sol's headphones, Tess's magnifying glass), in 3D and in every small avatar. Faces have mouths. |
| Worker profiles | None of their own. The line in the chief's Team list is enough. |

## The roles

### Atlas (`# administration`)

As Phase 1's admin, renamed and moved: built in, can't be deleted, crowned, proposes and acts
only after the person's reply (the server's newest-proposal and person-replied checks stay). Its
proposals now describe a team: channel name, chief (name, look, soul summary, model), workers
(name, look, role, model), autonomy and budget. It also handles a chief's request for a new
worker, as a proposal the person answers. It never drives chats.

### The chief of staff

A Vision with a soul, one per channel. It wakes on the person's messages in its channel (no
@mention needed), its workers' reports, requests from other chiefs, its routines, and events the
person causes (Stop, take over, hand back). Its grant:

- **Reads:** the project (read-only, as Phase 1 Vision runs), its channel, its workers' threads
  and their work chats.
- **Team:** give a worker a task, redirect a worker, pause or resume a worker, and answer a
  worker's question within the channel's autonomy level.
- **Company:** send a request to another channel's chief; accept or decline one; ask Atlas for a
  worker; add, change or remove its own routines within its autonomy level.
- **Never:** start, send to or steer work chats itself; approve actions; change modes, settings,
  autonomy or budget; create or delete channels or agents.

### Workers

Visions without a soul. Each has a name, a look, a role (a short instruction text that shapes
its work) and a model. A worker wakes only on its chief's task or redirect, and on events from
its own work chats. It keeps the Phase 1 Vision grant: it drives only the chats it owns, never
approves, and stays inside the channel's autonomy and budget. It reports to its chief, never to
the person. A report can be pushback: a reason and a safer way.

### The person

Talks to each chief. Sees every worker's thread and work chats read-only from the channel, and
can still open, stop, take over and hand back any work chat as in Phase 1. Approvals stay the
person's alone, as do the two locked autonomy rows.

## How work flows

1. The person writes in `# release`. Juno wakes.
2. Juno answers what she can from reading. For a change she gives a worker a task. The task and
   the worker's replies form that worker's thread with Juno.
3. Rook works in his own chats, then reports back, or pushes back before doing it.
4. Juno decides and tells the person what matters, in plain words. "Done" still needs evidence
   the reporting agent read itself, and Juno checks a worker's evidence before passing it on.
5. A question from a work chat goes to its worker; if the channel's level doesn't cover it, to
   the chief; if not the chief's either, to the person in the channel. Approvals go straight to
   the person, shown in the channel by the chief.

## Between channels

`# release`'s chief can ask `# api`'s chief for something ("fix the export date format before
Friday"). The request lands as a wake for the receiving chief, who accepts or declines within its
autonomy. Each channel shows one line ("Asked `# api` to …", "`# release` asked us to …") that
opens the exchange. A daily cap on requests sent by each channel (configurable, see below) stops
two chiefs from bouncing work.

## Routines

A routine is a schedule (weekdays at 09:00, Fridays at 15:00) plus a standing instruction. When
it fires, the chief wakes as if the person had written that instruction. Routines spend the
channel's budget and wait when it is used up. Company-wide **quiet hours** in Settings › Visions
stop routines and other agent-started wakes; the person's own messages always go through. A
chief that adds or changes a routine posts one line in the channel. The chief's profile lists the
routines with an on/off switch and Remove. Phase 1's separate "heartbeat" is dropped: a morning
summary routine does that job. Routines are a Visions-only timer, separate from Automations.

## Autonomy and budget

Both belong to the **channel**, set in channel details and shown on the chief's profile. The
chief and every worker share them. Per-agent overrides from Phase 1 go away. The global ceiling
in Settings › Visions stays, and the two locked rows stay the person's under every level.

## Souls

`soul.md`, one per chief and one for Atlas, stored with the agent in the app-data `visions/`
store (the person's own, not the project's). The profile's Soul box edits the same file;
"Open soul.md" opens it in the person's chosen editor, as Settings › Permissions does for
`runtime.jsonc`. A change applies from the agent's next wake; a running wake keeps what it
started with. The file is bounded in size and an unreadable file shows a problem on the profile
instead of being ignored.

## What the person sees

- **Sidebar:** project switcher, then Channels: `# administration` pinned first, then the
  project's channels, each with its chief's small cloud. No separate Visions list.
- **Channel header:** the chief in 3D, about 130 px, with a soft glow and ground shadow; the
  channel name large in the display font with the channel's gradient; the chief's name, "Chief of
  staff" and its state; one chip per worker (cloud, name, role, status dot). Tapping a chip opens
  that worker's thread with the chief, read-only, with links to its work chats.
- **Channel:** only the person and the chief, plus the one-line notes for requests between
  channels and new routines. The thinking row shows the chief's three-dot wave. The box says
  "Message <chief>" and has no @mention button.
- **Expressions,** each tied to a real state: Calm, Working, Done, Needs you, Stuck, Paused. Every
  state also has a text label.
- **Chief's profile** (from the header): 3D chief, name, "Chief of staff, `# channel`", state,
  Pause; Soul with Open soul.md; Model; the channel's Autonomy and Budget with today's spend;
  Team (each worker's look, role, model, status); Routines.
- **`# administration`:** Atlas in 3D with its crown, proposal cards for teams and new workers,
  "Message Atlas", no mode shield.
- No hint text anywhere, as before.

## The 3D characters

The clouds are already unions of circles, so each character is built in code as a union of
spheres, with its accessory from simple shapes (Claude's call): no art files and no art pipeline.
They render with WebGL inside the app on macOS, Windows and Linux, from a 3D library bundled with
the app (no download at run time). Expressions are animated face shapes (eyes, brows, mouth,
cheeks) on the front of the body. The scene renders only while the channel is on screen and
stops when hidden. With reduced motion, or where WebGL is unavailable, the header shows a still
picture of the current expression. Small avatars (sidebar, chips, messages) stay flat drawings
with the same colour and accessory. A frame-time check on the slowest supported machine is part
of the live check.

## What changes from Phase 1

Removed outright (no backward compatibility, per the owner's rule): `# hq` (now
`# administration`); channels with several peer Visions and @mention routing; the Visions section
of the sidebar; per-Vision autonomy and budget overrides; heartbeat. Phase 1's existing channels
are converted once on upgrade: the first member becomes the chief, the others workers, and the
person is told in `# administration`. Everything else from Phase 1 stays: the Vision grant and
ownership checks, bounded wakes, Stop, take over and hand back, server-enforced budgets, the
newest-proposal and person-replied checks, the plain voice, Vision chats marked in plain chat.

## Configuration and diagnostics

New fields in the `visions` block of `runtime.jsonc`, written field by field from Settings ›
Visions: quiet hours and the daily cap on requests between channels. Teams, roles, routines and
souls are content in the app-data `visions/` store, revision-checked, written only through store
APIs. Diagnostics stay metadata only: agent ids, channel ids, wake reasons, task, request and
routine ids, outcomes; never message, task or soul text.

## Proof

- **Server and CLI tests:** a chief cannot drive work chats; a worker acts only on its chief's
  tasks and its own chats; only a channel's chief can task its workers; requests between
  channels stop at the cap; a routine never fires in quiet hours or over budget; a chief cannot
  raise autonomy or budget through a routine or a request; only Atlas creates agents, and only
  after the person's reply; soul edits apply at the next wake.
- **Eval:** extend `evals/visions` with a team arm: the same tasks done by a chief and workers,
  compared with a Phase 1 single Vision and with goal mode. Measures as before (person messages,
  false "done", solved, cost), plus messages between agents per task and requests between
  channels. The redesign ships when the team needs no more person messages than a single Vision,
  with no false "done" claims, at a cost the owner accepts.
- **Scale:** worker threads, request lines and routine history grow with long use and get a
  scale test that counts work at two sizes.
- **Live check:** rows for the team channel header, a worker thread, the chief's profile,
  `# administration`, routines and the 3D fallback.

## Settled by Claude after the builder check (2026-11-13)

These are Claude's calls, not the owner's; the owner can overturn any of them.

1. **Bounded teamwork.** Phase 1's cap on runs in a row without a person message becomes one
   count per channel, shared by the chief and its workers (every agent wake counts, routines and
   requests from other channels included). At the cap the server stops starting wakes in that
   channel and posts one notice; the chief does not need a model call to say it. The person's
   next message resets the count.
2. **Questions.** The server enforces the level as in Phase 1: an `answer_question` above the
   channel's level is refused and the question stays open. The worker answers its own chats'
   questions when allowed; otherwise it passes the question to its chief in a report, and the
   chief answers when allowed. A question no agent may answer is shown in the channel by the
   server, attributed to the chief, so it reaches the person even if a model forgets to pass it
   on. Which decision row a question falls under stays the model's judgement, measured by the eval.
3. **The chief's calls** are `themis` methods on its grant, like Phase 1's (no new agent tools):
   give a task, redirect, pause and resume a worker, read a worker's thread and chats, answer a
   question in a worker's chat within the level, send, accept or decline a request between
   channels, ask Atlas for a worker, and add, change or remove its routines. Everything else is on
   the refusal list, with tests.
4. **Budget at zero.** The server stops every wake in the channel and posts the notice itself,
   once, as Phase 1 does. A request from another channel wakes the receiving chief, and that wake
   is paid by the receiving channel even when it declines.
5. **Take over and hand back** act on the worker that owns the chat. Hand back wakes that worker,
   and the chief gets the event as a report.
6. **Atlas's proposals** describe the whole team, and the server creates exactly what the
   proposal says, field by field. "Yes, but use a cheaper model" makes Atlas post a new proposal
   and ask again (the newest-proposal check already requires this). A chief's request for a worker
   wakes Atlas and counts toward `# administration`'s cap.
7. **Routine limits.** At most 10 routines per chief and no routine more often than once an hour,
   both in the `visions` block. The chief can't change these limits.
8. **3D ships in the same release, in its own PR, last.** The team logic and UI land first with
   the still picture, so the WebGL work (and WebKitGTK on Linux) can't hold up the rest.

## Left to the builder

Exact method names and payloads; how tasks and reports are stored (a worker thread is a bounded
log like a channel log); the routine schedule format; the converted names on upgrade; the 3D
library choice (it must be bundled and work on all three OSes); lighting and sphere layout per
character, matched to the canvas.
