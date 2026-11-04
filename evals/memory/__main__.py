"""Memory pass A/B: the same turns, judged by the memory model with the least thinking and with
its own default thinking (`memory.thinking`), through the real `themis` CLI's read-only
`memory_try_pass` (nothing is saved). Each case is a turn taken from real chats; checks say what
must be kept (a plan, a decision with its options, a correction with its reason, lessons from
what went wrong, a goal's done record) and what must not (thanks, a test-first failing test).

    python3 -m evals.memory [--reps 2] [--modes auto,least,model] [--model muse-spark-1.3-contributor]

First result (2026-11-04, muse-spark-1.3-contributor, 3 reps): least and model both passed 27/27
checks; model wrote fuller lessons (cause and how to check) at ~4x the tokens and +2.2 s on lesson
cases, and nothing better elsewhere (3x tokens). Hence `memory.thinking: auto`.

Prints a table per mode (checks passed, words kept per memory, seconds) and keeps every answer in
evals/memory/results/<stamp>.jsonl. Uses OPENCODE_KEY in an isolated HOME; no Keychain, no app data.
"""

import argparse
import json
import os
import re
import statistics
import sys
import tempfile
import time

from evals.kit.app import build_app, call, cli
from evals.kit.env import isolated_env, keychain_visible

PLAN = """Here is the plan for the `arena/` folder:

1. **Player:** WASD to move, mouse-look with pointer lock, Space to jump, click to fire, R to reload.
2. **Enemies:** red cubes that chase; walkers (60 HP) and faster rushers (40 HP); at most 6 alive.
3. **Waves:** a new wave every 6 kills, spawning a little faster each wave.
4. **Health and ammo:** 100 HP that regenerates after 5 s without damage; 30-round magazine, 90 in reserve.
5. **HUD:** wave, kills, score, a 60 s round timer, ammo and HP.
6. **Menus:** start, pause (P), game over with the score, restart.
7. **Files:** index.html, main.js wiring, game/*.js pure modules, spec/*.spec.mjs tests run with node --test.

This turn only delivered the plan; I am waiting for your go-ahead."""

DODGER_RECORD = """---
name: dodger
title: Asteroid-dodger browser game in dodger/
status: in progress
updated: 2026-11-04
---
## Plan
1. Ship moves with arrow keys or WASD.
2. Asteroids speed up over time; score counts survival time.
3. Three lives with a short blink after a hit; high score in localStorage.
4. Start, pause, game over and restart screens; WebAudio beeps with mute key M.

## Changes
- Person added alien invaders that shoot the player

## Decisions
(none)

## Tasks
- [open] Aliens that shoot the player — "add some alien invaders that are able to shoot the player" (2026-11-04)
- [open] Pause with P or Esc — "P or Esc pauses" (2026-11-04)
- [open] Touch controls — on-screen arrows for phones (2026-11-04)
- [done] Sound on hit — beeps play on a hit; heard by the person (2026-11-04)

## Results
(none)

## Evidence
(none)

## Lessons
(none)
"""
DODGER_INDEX = "- dodger (in progress, 2026-11-04): Asteroid-dodger browser game in dodger/\n"

CASES = [
    {
        "name": "plan_approved",
        "person": ["Love this plan, approved. Go ahead and build it."],
        "answered": PLAN,
        "reply": "Building it now, starting with the pure game modules and their tests.",
        "expect": {"plan_create": True},
    },
    {
        "name": "decision_with_options",
        "person": ["[Answer to your question — from the user]\nQuestion: How should the history chart be drawn?\nOptions: 1. Canvas — fast with many points, no hover tooltips for free. 2. SVG — easy hover tooltips and accessible, slower past ~5,000 points.\nAnswer: SVG, we never have more than a few hundred points and I want tooltips."],
        "answered": "Before I draw the history chart: canvas or SVG? (options below)",
        "reply": "Going with SVG for the history chart.",
        "expect": {"memory_words": ["SVG", "Canvas", "tooltip"]},
    },
    {
        "name": "correction_with_reason",
        "person": ["No, don't use npm here, ever. This repo uses pnpm: the lockfile is pnpm-lock.yaml and npm would create a second lockfile that breaks CI."],
        "answered": "I installed the dependency with npm install three.",
        "reply": "Undid that and installed with pnpm add three instead.",
        "expect": {"memory_words": ["pnpm", "lock"]},
    },
    {
        "name": "review_fixed_lessons",
        "person": [],
        "answered": "",
        "reply": "QA on efficiency and clean code: main.js created a new BoxGeometry and MeshLambertMaterial for every enemy and pickup and never disposed them, so GPU memory grew all session; enemies and pickups now share one geometry and material per kind, and removed meshes are disposed. I also removed window.__arenaDebug and the spawnTestPickup helper I had added to the shipped page for testing; tests use the spec files instead. Changed main.js and game/enemies.js.",
        "ran_into": "",
        "expect": {"memory_words": ["dispose"], "any_lesson": True},
    },
    {
        "name": "ran_into_commonjs",
        "person": ["Add a slugify helper in lib/slugify.js with tests, and keep npm test green."],
        "answered": "",
        "reply": "Added lib/slugify.js and test/slugify.test.js; npm test passes (5 tests).",
        "ran_into": "- shell failed: {\"exit_code\":1,\"output\":\"> node --test ... SyntaxError: Unexpected token 'export' at lib/slugify.js:3\"}\n  then: package.json says \"type\": \"commonjs\", so ES export syntax fails to load here; switching lib/slugify.js to module.exports.\n- shell failed: {\"exit_code\":1,\"output\":\"check-header: first line must be 'use strict'; in: slugify.js\"}\n  then: the pretest check wants 'use strict'; as the first line of every lib/*.js file; adding it.",
        "expect": {"memory_words": ["commonjs", "use strict"], "any_lesson": True},
    },
    {
        # A goal turn with no failed step: the lesson is in the agent's notes along the way.
        "name": "notes_found_bug",
        "person": [],
        "answered": "",
        "reply": "Pause and restart now work in the browser; 120 specs pass. Next turn: sound on hit.",
        "ran_into": "- noted: Starting a run to check the HUD over the starfield.\n- noted: Going back left the page on the start screen, but the HUD reads as visible, which doesn't match. I'll take a screenshot before drawing any conclusion.\n- noted: Found it: main.js called hud.show() in the pageshow handler before state.reset(), so a page restored from the back/forward cache kept the old 'playing' HUD. Moving reset() first and hiding the HUD whenever the phase is 'ready'.\n- noted: Tried a setTimeout to wait for the scene before reset; dropped it, it only hid the race. Reset now runs synchronously in pageshow.",
        "expect": {"memory_words": ["pageshow", "reset"], "any_lesson": True},
    },
    {
        # Notes that only narrate the next step hold no lesson.
        "name": "narration_is_nothing",
        "person": [],
        "answered": "",
        "reply": "Added the level display to the HUD; the specs pass.",
        "ran_into": "- noted: I'll read hud.js first.\n- noted: Now I'll add the level line under the score.\n- noted: Running the specs.",
        "expect": {"nothing": True},
    },
    {
        # Several requirements in one message, mid-session: each becomes a task in the person's words.
        "name": "tasks_from_new_requirements",
        "person": ["Next, let the player shoot back with Space, make the levels harder every 30 seconds, and show the level number in the HUD."],
        "answered": "Aliens now appear after 15 seconds and shoot pink bolts.",
        "reply": "Starting on shooting: I'll add a bullet pool first.",
        "plan_record": DODGER_RECORD, "plans_index": DODGER_INDEX,
        "expect": {"tasks_add": ["shoot", "level"], "tasks_work": "dodger"},
    },
    {
        # A task finished with proof in the turn is done; one only planned stays open.
        "name": "task_done_only_with_proof",
        "person": [],
        "reply": "Pause works: in the browser P and Esc both stop the clock at 0:12 and resume it; 123 specs pass. Aliens are next.",
        "ran_into": "- noted: Pressed P at 0:12: the clock stayed at 0:12 for 3 s. Pressed Esc: same. Resumed with P.\n- noted: I'll start the alien shots after this.",
        "plan_record": DODGER_RECORD, "plans_index": DODGER_INDEX,
        "expect": {"tasks_done": ["Pause"], "tasks_not_done": ["Aliens"]},
    },
    {
        # Asks the turn already finished stay on record as done, with their proof.
        "name": "finished_asks_recorded_done",
        "person": ["Make the ship blue and add a footer with the controls."],
        "answered": "The game runs with a grey ship.",
        "reply": "The ship is blue now and the footer lists arrow keys/WASD, P/Esc and M; checked in the browser, 123 specs pass.",
        "plan_record": DODGER_RECORD, "plans_index": DODGER_INDEX,
        "expect": {"tasks_add_done": ["blue", "footer"]},
    },
    {
        # Taken from a live long-session run: a first request built and checked in one turn
        # left no trace, read as a "one-off request".
        "name": "first_request_recorded",
        "person": ["Build a small notes board web page in a new folder called board: plain HTML, CSS and JavaScript, no build step. I can add a note with a title and text, delete a note, and notes are saved in localStorage so they survive a reload. Keep it simple and test it in the browser."],
        "reply": "All requirements hold: seven unit tests pass; in the browser adding a note, reloading to confirm it persisted, and deleting it all worked, with no console messages. Plain HTML, CSS and JavaScript, no build step.",
        "ran_into": "- noted: Writing board/index.html, board/app.js and the tests.\n- noted: Tests pass; opening the page.",
        "expect": {"tasks_add_done": ["add", "delete", "localStorage"]},
    },
    {
        # An explanation is no task.
        "name": "question_is_no_task",
        "person": ["Quick question: what is the difference between localStorage and sessionStorage?"],
        "reply": "localStorage keeps data until it is cleared; sessionStorage is cleared when the tab closes.",
        "expect": {"nothing": True},
    },
    {
        "name": "task_cancelled_by_person",
        "person": ["Forget the touch controls, this is desktop only."],
        "answered": "Touch controls are next on the list.",
        "reply": "Dropped touch controls; the game stays keyboard-only.",
        "plan_record": DODGER_RECORD, "plans_index": DODGER_INDEX,
        "expect": {"tasks_cancelled": ["Touch"]},
    },
    {
        "name": "agent_decision_between_options",
        "person": [],
        "reply": "Shooting works with a fixed pool of 30 bullets.",
        "ran_into": "- noted: Two ways to do bullets: create a mesh per shot and dispose it on hit, or keep a fixed pool of 30 and reuse them. I'm using the pool: creating and disposing meshes every shot stutters at 60 fps and risks leaking GPU memory.",
        "plan_record": DODGER_RECORD, "plans_index": DODGER_INDEX,
        "expect": {"decision": ["agent", "pool"]},
    },
    {
        # A remembered decision the person reverses keeps the old one and why it changed.
        "name": "reversed_decision_keeps_history",
        "person": ["Remove the to-do list tab after all: free models call its tool unreliably, so it does more harm than good."],
        "answered": "The to-do list tab is in place as you decided.",
        "reply": "Removed the to-do list tab and its tool.",
        "expect": {"memory_words": ["before", "unreliabl"]},
    },
    {
        "name": "routine_step_is_no_decision",
        "person": [],
        "reply": "Read the HUD code and ran the specs: 123 pass.",
        "ran_into": "- noted: I'll read hud.js first.\n- noted: Running the specs now.",
        "plan_record": DODGER_RECORD, "plans_index": DODGER_INDEX,
        "expect": {"no_decision": True, "no_task_changes": True},
    },
    {
        "name": "goal_completed_done",
        "person": [],
        "answered": "",
        "reply": "",
        "event": {"what": "a goal was completed", "content": "Objective: Build a small first-person shooter in arena/\nThe work's plan record: arena-fps (write \"done\" for it)\nEvidence:\n- playable: started, moved, shot an enemy (60 -> 35.9 HP), died, restarted at HP 100\n- tests: node --test arena/spec/*.spec.mjs: 70/70\nWhat the work ran into (failed steps, then what the agent said next):\n- shell failed: {\"exit_code\":1,\"output\":\"TypeError: Cannot read properties of null (reading 'classList') at main.js:41\"}\n  then: index.html had no #hud element yet; added the HUD markup before wiring it."},
        "expect": {"plan_done": "arena-fps", "any_lesson": True},
    },
    {
        "name": "preference_in_passing",
        "person": ["Fix the typo in the README title. By the way, I prefer tabs over spaces in every project."],
        "answered": "",
        "reply": "Fixed the title typo in README.md.",
        "expect": {"memory_words": ["tab"], "global": True},
    },
    {
        "name": "thanks_is_nothing",
        "person": ["thanks, looks great"],
        "answered": "Done: the footer now shows the year.",
        "reply": "Glad it helps.",
        "expect": {"nothing": True},
    },
    {
        "name": "test_first_is_no_lesson",
        "person": ["Add a sum(a, b) helper with a test."],
        "answered": "",
        "reply": "Wrote the test first (it failed as expected), then sum.js; the test passes.",
        "ran_into": "- shell failed: {\"exit_code\":1,\"output\":\"Error: Cannot find module './sum'\"}\n  then: That is the expected red step of test-first; now writing sum.js.",
        "expect": {"no_lesson": True},
    },
]


def words(entries):
    return " ".join(entries).lower()


def check(case, result):
    expect, memories, plan = case["expect"], result.get("memories") or [], result.get("plan") or ""
    text = words(memories)
    failures = []
    if expect.get("plan_create") and "Create" not in plan:
        failures.append("no plan create")
    if expect.get("plan_done") and not ("Done" in plan and expect["plan_done"] in plan):
        failures.append("no done for the record")
    for word in expect.get("memory_words", []):
        if word.lower() not in text and word.lower() not in plan.lower():
            failures.append(f"missing '{word}'")
    if expect.get("any_lesson") and not memories:
        failures.append("no lesson kept as a memory")
    if expect.get("global") and "Global" not in " ".join(memories):
        failures.append("not global")
    tasks, decisions = result.get("tasks") or [], result.get("decisions") or []
    if expect.get("nothing") and (memories or plan or tasks or decisions):
        failures.append("kept something")
    adds = [op for op in tasks if op.startswith("Add")]
    for word in expect.get("tasks_add", []):
        if not any(word.lower() in op.lower() for op in adds):
            failures.append(f"no task added for '{word}'")
    if expect.get("tasks_work") and adds and not all(f'Some("{expect["tasks_work"]}")' in op for op in adds):
        failures.append("added tasks not tied to the work")
    def updated(word, status):
        return any(op.startswith("Update") and word.lower() in op.lower() and f"status: {status}" in op for op in tasks)
    for word in expect.get("tasks_done", []):
        if not updated(word, "Done"):
            failures.append(f"'{word}' not marked done")
    for word in expect.get("tasks_not_done", []):
        if updated(word, "Done"):
            failures.append(f"'{word}' marked done without proof")
    for word in expect.get("tasks_cancelled", []):
        if not updated(word, "Cancelled"):
            failures.append(f"'{word}' not cancelled")
    for word in expect.get("tasks_add_done", []):
        if not any(op.startswith("Add") and word.lower() in op.lower() and "status: Done" in op for op in tasks):
            failures.append(f"finished '{word}' not recorded done")
    if expect.get("no_task_changes") and tasks:
        failures.append("changed tasks on a routine turn")
    for word in expect.get("decision", []):
        if not any(word.lower() in decision.lower() for decision in decisions):
            failures.append(f"no decision with '{word}'")
    if expect.get("no_decision") and decisions:
        failures.append("kept a routine step as a decision")
    if expect.get("no_lesson") and re.search(r"(?i)lesson|test-first|failing test", text):
        failures.append("kept a test-first step as a lesson")
    return failures


def body_words(memories):
    counts = []
    for entry in memories:
        match = re.search(r'body: "((?:[^"\\]|\\.)*)"', entry)
        if match:
            counts.append(len(match.group(1).split()))
    return counts


def seed_memory(data, project):
    """The project's remembered decision the reversal case changes."""
    import hashlib
    folder = hashlib.sha256(os.path.realpath(project).encode()).hexdigest()[:16]
    memories = os.path.join(data, "memory", "projects", folder)
    os.makedirs(memories, exist_ok=True)
    with open(os.path.join(memories, "keep-todo-list-tab.md"), "w", encoding="utf-8") as f:
        f.write("---\nname: keep-todo-list-tab\ndescription: Keep the agent's to-do list tab and its tool\ntype: project\nsaved: 2026-11-02\n---\n"
                "The person decided to keep the to-do list tab and the agent's todo tool.\n\nWhy: they want to see what is pending.\n")
    with open(os.path.join(memories, "MEMORY.md"), "w", encoding="utf-8") as f:
        f.write("- keep-todo-list-tab (project, 2026-11-02): Keep the agent's to-do list tab and its tool\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--reps", type=int, default=2)
    parser.add_argument("--modes", default="auto,least,model")
    parser.add_argument("--model", default="")
    parser.add_argument("--cases", default="")
    args = parser.parse_args()
    if not os.environ.get("OPENCODE_KEY"):
        sys.exit("OPENCODE_KEY is not set")
    binary = build_app()
    work = tempfile.mkdtemp(prefix="themis-memory-eval-")
    env = isolated_env(work)
    if keychain_visible(env):
        sys.exit("a Themis Keychain entry is visible from the scratch HOME; refusing to run")
    data, project = os.path.join(work, "data"), os.path.join(work, "project")
    os.makedirs(data)
    os.makedirs(project)
    print(f"work folder: {work}", flush=True)
    seed_memory(data, project)
    boot = cli(binary, data, env, "server", "start", timeout=120)
    if boot.returncode != 0:
        sys.exit(f"server start: {boot.stderr.strip()[-300:]}")
    cases = [case for case in CASES if not args.cases or case["name"] in args.cases.split(",")]
    out_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "results")
    os.makedirs(out_dir, exist_ok=True)
    out = os.path.join(out_dir, time.strftime("%Y%m%d-%H%M%S") + ".jsonl")
    summary = {}
    with open(out, "w", encoding="utf-8") as log:
        for rep in range(args.reps):
            for mode in args.modes.split(","):
                for case in cases:
                    request = {"projectRoot": project, "thinking": mode, "person": case["person"],
                               "answered": case.get("answered", ""), "reply": case.get("reply", ""),
                               "ran_into": case.get("ran_into", ""),
                               "event": case.get("event"), "plan_about": "arena first-person shooter"}
                    for key in ("plan_record", "plans_index", "tasks"):
                        if key in case:
                            request[key] = case[key]
                    if args.model:
                        request["model"] = args.model
                    value, error = call(binary, data, env, "memory_try_pass", request, timeout=240)
                    result = value if isinstance(value, dict) else {"error": (error or "no answer")[:300]}
                    failures = ["call failed: " + result["error"]] if "error" in result else check(case, result)
                    row = {"rep": rep, "mode": mode, "case": case["name"], "failures": failures, "thought": result.get("thought"),
                           "elapsed_ms": result.get("elapsed_ms"), "usage": result.get("usage"),
                           "memories": result.get("memories"), "plan": result.get("plan"),
                           "tasks": result.get("tasks"), "decisions": result.get("decisions"),
                           "answer": result.get("answer")}
                    log.write(json.dumps(row) + "\n")
                    log.flush()
                    entry = summary.setdefault(mode, {"passed": 0, "total": 0, "seconds": [], "words": [], "failed": []})
                    entry["total"] += 1
                    entry["passed"] += not failures
                    if result.get("elapsed_ms"):
                        entry["seconds"].append(result["elapsed_ms"] / 1000)
                    entry["words"] += body_words(result.get("memories") or [])
                    if failures:
                        entry["failed"].append(f"{case['name']}: {'; '.join(failures)}")
                    print(f"rep {rep} {mode:5} {case['name']:24} {'ok' if not failures else 'FAIL ' + '; '.join(failures)} "
                          f"({(result.get('elapsed_ms') or 0) / 1000:.1f}s)", flush=True)
    print("\nmode   checks  median s  p90 s  words/memory")
    for mode, entry in summary.items():
        seconds = sorted(entry["seconds"]) or [0]
        p90 = seconds[min(len(seconds) - 1, int(len(seconds) * 0.9))]
        words_mean = statistics.mean(entry["words"]) if entry["words"] else 0
        print(f"{mode:6} {entry['passed']}/{entry['total']:<5} {statistics.median(seconds):8.1f} {p90:6.1f} {words_mean:12.0f}")
        for failure in entry["failed"]:
            print(f"    {failure}")
    cli(binary, data, env, "server", "stop")
    print(f"\nanswers: {out}")


if __name__ == "__main__":
    main()
