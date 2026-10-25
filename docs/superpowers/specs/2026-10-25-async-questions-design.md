# Questions that don't stop the work

Date: 2026-10-25 · Branch: `codex/async-questions` (design on
`codex/async-questions-design`) · Builds on goal mode
([2026-10-18-goal-mode-design.md](2026-10-18-goal-mode-design.md)) and on
"goals never give up" (PR #98).

## Why

When the agent needs the person's input today, it asks in its answer and the
turn ends. In goal mode the check then calls it `needs_user` and the goal
pauses until a reply. Work stops on every question, even one the agent could
have worked around. The owner wants the opposite: a question box the person
can answer whenever they like, while the agent keeps working.

Owner's words (2026-10-25): include "a box or question box if the agent need
more clarification or require input from user. However, it would need to be
asynchronous, so that the agent can continue working while waiting, if input
is not provided, it will minimize or hide the box inline conversation, which
can be answered later"; "it works both in goal and normal chat"; questions
"shouldn't be a stopper unless it is truly blocking, which is critical for the
goal to proceed further".

## Goals

1. The agent can ask the person a question **without ending its turn**. It
   says what it will assume meanwhile and carries on.
2. The question shows as a **box inline in the conversation**: the question,
   optional choices, a free-text answer, and the assumption the agent is
   working on.
3. **Unanswered boxes collapse** once the agent moves on (its next step, or the
   end of the turn) into a one-line "Open question" row. The person can expand
   and answer it at any time later, in the same chat.
4. An answer **reaches the agent at once if a run is going** (like a steer),
   otherwise it starts the next turn (normal chat) or rides on the next goal
   turn (goal mode). The agent then revisits work done on a wrong assumption.
5. **Works in goal mode and normal chats alike.**
6. Only a **truly blocking** question stops work: one where nothing more can be
   done toward the request or goal without the answer. Then the turn ends and,
   in goal mode, the goal waits for the person (today's `waiting`).

## Non-goals

- No timeouts and no limit on how long a question stays open. An open question
  never expires; the agent keeps its stated assumption until answered.
- Not a replacement for approvals. Tool approvals (Allow once / in this chat /
  Deny) keep their own flow and still block the tool call.
- No question boxes from the goal check: only the agent asks.
- No notification per non-blocking question (a blocking one uses the existing
  "waiting for your answer" goal notification).

## Anti-goals (a failure even if it works)

- Work stalls or a goal pauses because of a question the agent could work
  around.
- The chat fills with boxes: repeated or near-identical questions each adding
  a box.
- The agent asks without saying what it assumes, so work done meanwhile is a
  guess the person can't see.
- An answer is silently lost (sent while no run is going and never delivered).

## Behaviour

### The tool

A new agent tool, `ask_user`, available in every chat (approval-free like
`set_goal`; a deny rule still wins):

```jsonc
{"question": "Should totals include retired items?",
 "choices": ["Yes", "No"],            // optional, at most a handful
 "assumption": "Yes, include them",   // required unless blocking
 "blocking": false,                    // default false
 "topic": "index-total"}              // optional short key, see below
```

- Non-blocking: returns at once with an id and a note (prompt file) telling the
  agent to continue on its assumption and that the answer will arrive as a
  message. The run continues.
- Blocking: the tool returns, and the agent is told to end its turn with the
  question. The run's end leaves the chat waiting; in goal mode the goal goes
  to `waiting` (no check-model guess needed: the tool says so).
- `topic`: asking again on a topic that is still open updates that question
  (new wording or assumption) instead of adding a box. Without a topic, an
  identical open question is also merged.

### The box

- Inline at the point it was asked, styled as a question, not as an error or a
  goal note. Shows the question, choices as buttons, a text field, and "Working
  on: <assumption>".
- Collapses to a single row "Open question · <question>" when the run takes its
  next step or ends. Expanding it shows the full box again.
- The goal bar (goal mode) and the chat header show a small count of open
  questions; clicking it scrolls to the oldest.
- Answered: the box shows the answer and stays collapsed, read-only.
- Follows DESIGN.md "Only what earns its place": no extra labels or controls.

### Delivering the answer

- Saved first (app data, per chat, beside the queue), then delivered.
- A run is going: injected like a steer, read before the agent's next step, as
  a runtime-marked message quoting the question, the assumption and the answer.
- No run is going, normal chat: sent as the next message (it shows in the chat
  as the person's answer to that question).
- No run is going, goal mode: goes into the next goal turn's message; if the
  goal is `waiting` on that question, the answer resumes it.
- The agent's instructions (prompt files) say: when an answer differs from
  your assumption, find and fix what you did on the wrong assumption first.

### Goal mode

- `needs_user` from the goal check stays only as a fallback for an agent that
  asked in plain text; with the tool, a blocking question sets `waiting`
  directly and non-blocking questions never pause.
- Open questions are listed briefly in the goal turn message ("Open questions:
  … (assuming …)") so they survive checkpoints.
- **A goal can be reached with questions still open** (owner's decision,
  2026-10-25). The agent's "goal reached" message lists each open question and
  the assumption it used; the boxes stay open. Answering one later starts a
  normal turn that fixes anything built on a wrong assumption.

## Constraints

- Model-facing text (tool description, results, notes, the answer message) in
  `crates/core/builtins/prompts/*.md` with constants and inventory tests.
- Storage: app-data `questions/<chat id>.json`, bounded (count and size), a
  damaged file ignored with a fixed diagnostics event; removed with the chat.
- Diagnostics: question ids, blocking flag, delivered/merged/answered events;
  never the question or answer text.
- Durable history keeps the question and the answer, so a chat reopened later
  still shows boxes and their state.
- Shared app server commands (`list_questions`, `answer_question`) so the CLI
  and desktop behave the same; a CLI end-to-end case and browser/native UI
  checks.
- Answers grant nothing: an answer cannot approve a tool or change a rule.

## Evaluation

- Scripted (CI): a non-blocking question keeps the run going; an answer during
  a run arrives before the next step; an answer after the run starts the next
  turn / rides on the next goal turn; a blocking question sets `waiting`; a
  repeated topic merges.
- Goal soak: count questions asked and blocking ones; a run must keep working
  after a non-blocking question. A later soak variant can answer questions
  with a script to measure whether the agent fixes work done on a wrong
  assumption.

## Decisions made in review (designer's calls, not the owner's)

- **Source of truth:** `questions/<chat id>.json` holds question state; the
  transcript records new `question_asked` / `question_answered` events for
  display. If the file is damaged, it is ignored (diagnostic) and the
  transcript events rebuild it. The new events add no parsed-back markers to
  existing ones.
- **Delivery never loses an answer:** the answer is saved as pending first.
  If a run is going, it goes into that run's steer inbox under the queue lock
  (the same handoff steers use). Otherwise, or if the run ends before reading
  it, it stays pending and goes with the next message or goal turn. It is
  marked delivered only once a run has read it.
- **Normal chat, no run going:** an answer starts a turn at once, ahead of
  queued prompts. Answers given before that turn reads them go out together in
  one message. A draft in the composer is untouched.
- **Waiting goal:** only the answer to the blocking question resumes it; other
  open questions stay open. A blocking question may be asked while
  non-blocking ones are open.
- **Who may ask:** only the chat's own runs. Subagents, background tasks and
  watches do not get `ask_user`.
- **At the bound:** the tool refuses with a note telling the agent to keep
  working on its assumption. Re-asking an answered topic opens a new box.
- **Blocking is enforced:** after a blocking `ask_user` the runtime ends the
  run once the tool result is delivered; the agent gets no further tool calls.
- **CLI and headless:** questions persist in app data like the queue; a CLI
  run ends normally with open ones, `list_questions` shows them, and
  `answer_question` answers them from a later session.

## Left to the builder

- Exact UI layout and wording within DESIGN.md; mockups before building.
- The bound on open questions per chat and choice count.
- How the merged question is shown when its wording changes.
