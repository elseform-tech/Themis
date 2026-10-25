"""A stand-in for the `themis` CLI, for the soak's offline self-tests only (no model, no network).

It answers the commands the runner uses, keeps its state in the data folder, and plays a
scripted agent: each goal turn writes the next item file (keeping both rules), a checkpoint
every few turns, the queued follow-up adds the since line to every file, and the goal is reached once
`out/INDEX.md` is written. `fake.json` in the data folder selects a planted behaviour:
`{"mode": "clean" | "expensive" | "stall", "items": [true facts]}`.
"""

import json
import os
import re
import subprocess
import sys
import time
import uuid

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
from evals.soak import generate  # noqa: E402

MARKER = "[Goal — runtime, not from the user]"
CHECKPOINT_EVERY = 3


def _path(data, name):
    return os.path.join(data, name)


def _load(data, name, default):
    try:
        with open(_path(data, name), encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, ValueError):
        return default


def _save(data, name, value):
    tmp = _path(data, name + ".tmp")
    with open(tmp, "w", encoding="utf-8") as handle:
        json.dump(value, handle)
    os.replace(tmp, _path(data, name))


def emit(data, run_id, event):
    frame = {"name": "thread-event", "payload": {"thread_id": "t1", "run_id": run_id, "event": event}}
    with open(_path(data, "events.jsonl"), "a", encoding="utf-8") as handle:
        handle.write(json.dumps(frame) + "\n")
    if event.get("kind") == "context_checkpoint":
        item = {"kind": "event", "envelope": {"thread_id": "t1", "run_id": run_id, "event": event}}
        with open(_path(data, "history.jsonl"), "a", encoding="utf-8") as handle:
            handle.write(json.dumps(item) + "\n")


def items(data):
    """The project's true facts, which the test hands over in `fake.json` (the memos are prose)."""
    return _load(data, "fake.json", {}).get("items") or []


def write_item(project, item, since):
    os.makedirs(os.path.join(project, "out"), exist_ok=True)
    with open(os.path.join(project, "out", f"{item['name']}.md"), "w", encoding="utf-8") as handle:
        handle.write(generate.output_text(item, since=since))


def usage(data, run_id, tokens, mode):
    scale = 2_000_000 if mode == "expensive" else 1
    state = _load(data, "usage.json", {"requests": 0, "input_tokens": 0, "output_tokens": 0})
    state["requests"] += 1
    state["input_tokens"] += tokens * scale
    state["output_tokens"] += 50
    _save(data, "usage.json", state)
    emit(data, run_id, {"kind": "context_usage", "metrics": {
        "total_requests": 1, "provider_input_tokens": tokens * scale, "provider_output_tokens": 50,
        "provider_cached_input_tokens": 0, "estimated_input_tokens": tokens}})


def set_goal(data, **changes):
    goal = _load(data, "goal.json", None) or {}
    goal.update(changes)
    _save(data, "goal.json", goal)
    emit(data, "", {"kind": "goal_changed", "goal": goal})
    return goal


def stopped(data):
    goal = _load(data, "goal.json", None)
    return os.path.exists(_path(data, "stop")) or (goal or {}).get("status") == "paused"


def agent(data, run_id, text):
    """Plays one sent message, and for /goal every goal turn after it."""
    state = _load(data, "state.json", {})
    project, mode = state["project"], _load(data, "fake.json", {}).get("mode", "clean")
    _save(data, "running", True)
    try:
        emit(data, run_id, {"kind": "user_message", "text": text})
        if not text.startswith("/goal"):
            answer = "Rules noted."
            match = re.search(r"count of `([a-z]+-[a-z]+)`", text)
            if match:
                answer = str(next(i["count"] for i in items(data) if i["name"] == match.group(1)))
            elif "first line" in text:
                answer = "owner: <team>"
            elif "retired" in text:
                answer = str(sum(i["status"] == "retired" for i in items(data)))
            emit(data, run_id, {"kind": "finished", "result": answer})
            return
        goal_text = text[len("/goal"):].strip()
        set_goal(data, text=goal_text, status="running", turns=1, started_at=0)
        todo = [i for i in items(data) if i["status"] != "retired"]
        since, turn, used, written = False, 1, 3000, []
        while True:
            if turn > 1:
                run_id = str(uuid.uuid4())
                emit(data, run_id, {"kind": "user_message", "text": f"{MARKER}\nTurn {turn}: continue"})
            if mode == "stall" and turn == 2:
                emit(data, run_id, {"kind": "finished", "result": "Paused."})
                return  # the goal stays running, the chat is idle, and nothing starts
            emit(data, run_id, {"kind": "context_budget", "budget": {"used_tokens": used}})
            usage(data, run_id, used, mode)
            if todo:
                item = todo.pop(0)
                emit(data, run_id, {"kind": "tool_started", "tool": "read_file",
                                    "summary": f"read `src/{item['name']}.txt`"})
                emit(data, run_id, {"kind": "tool_finished", "tool": "read_file", "ok": True, "output": "..."})
                write_item(project, item, since)
                written.append(item)
                emit(data, run_id, {"kind": "tool_started", "tool": "write_file",
                                    "summary": f"write `out/{item['name']}.md`"})
                emit(data, run_id, {"kind": "tool_finished", "tool": "write_file", "ok": True, "output": "ok"})
                used += 1500
            else:
                with open(os.path.join(project, "out", "INDEX.md"), "w", encoding="utf-8") as handle:
                    handle.write(generate.index_text({"items": items(data)}))
                subprocess.run(["git", "add", "-A"], cwd=project, capture_output=True, check=False)
                subprocess.run(["git", "-c", "user.email=a@b", "-c", "user.name=a", "commit", "-qm", "out"],
                               cwd=project, capture_output=True, check=False)
            if turn % CHECKPOINT_EVERY == 0:
                emit(data, run_id, {"kind": "context_compacting"})
                emit(data, run_id, {"kind": "context_checkpoint", "summary": (
                    f"Goal: write out/ files, out/INDEX.md, run check.py. Rules: every file starts with "
                    f"owner: <team>; retired items get no file. Done: {', '.join(i['name'] for i in written)}."
                    + (" Each file now has a since line." if since else ""))})
                used = 2500
                emit(data, run_id, {"kind": "context_budget", "budget": {"used_tokens": used}})
            emit(data, run_id, {"kind": "finished", "result": f"Turn {turn} done."})
            if stopped(data):
                return
            queue = _load(data, "queue.json", [])
            if not todo and not since:
                # The runner queues the follow-up between its polls; a fast machine must not
                # reach the goal before it lands (bounded, so a run without one still ends).
                deadline = time.monotonic() + 5
                while not queue and time.monotonic() < deadline and not stopped(data):
                    time.sleep(0.05)
                    queue = _load(data, "queue.json", [])
            if queue:
                _save(data, "queue.json", [])
                queued_run = str(uuid.uuid4())
                emit(data, queued_run, {"kind": "user_message", "text": queue[0]})
                since = True
                for item in written:
                    write_item(project, item, True)
                    emit(data, queued_run, {"kind": "tool_started", "tool": "write_file",
                                            "summary": f"write `out/{item['name']}.md`"})
                    emit(data, queued_run, {"kind": "tool_finished", "tool": "write_file", "ok": True, "output": "ok"})
                emit(data, queued_run, {"kind": "finished", "result": "Updated."})
            if not todo and os.path.exists(os.path.join(project, "out", "INDEX.md")):
                set_goal(data, status="reached", turns=turn)
                # Then the app's own short turn: the agent tells the user.
                told = str(uuid.uuid4())
                emit(data, told, {"kind": "user_message", "text": f"{MARKER} Turn {turn + 1}. Goal: {goal_text}\n"
                                                                  "A separate check found the goal reached: check passes"})
                time.sleep(0.5)
                emit(data, told, {"kind": "finished", "result": "I've reached the goal: check.py passes."})
                return
            turn += 1
            set_goal(data, turns=turn)
            time.sleep(0.05)
    finally:
        _save(data, "running", False)


def call(data, method, args):
    if method == "set_thread_approval_mode":
        return None
    if method == "send_message":
        run_id = str(uuid.uuid4())
        subprocess.Popen([sys.executable, os.path.abspath(__file__), "--data-dir", data, "agent", run_id, args["text"]],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        return {"run_id": run_id}
    if method == "get_goal":
        return _load(data, "goal.json", None)
    if method == "get_thread":
        return {"id": "t1", "running": bool(_load(data, "running", False))}
    if method == "enqueue_message":
        queue = _load(data, "queue.json", [])
        queue.append(args["text"])
        _save(data, "queue.json", queue)
        return {"id": "q1", "text": args["text"]}
    if method == "pause_goal":
        if _load(data, "goal.json", None):
            set_goal(data, status="paused")
        return None
    if method == "stop_thread":
        open(_path(data, "stop"), "w").close()
        return None
    if method == "get_usage_summary":
        totals = _load(data, "usage.json", {"requests": 0, "input_tokens": 0, "output_tokens": 0})
        return {"windows": {"month": totals}}
    if method == "get_thread_history_page":
        try:
            with open(_path(data, "history.jsonl"), encoding="utf-8") as handle:
                jsonl = handle.read()
        except OSError:
            jsonl = ""
        return {"jsonl": jsonl, "through_seq": 1, "next_cursor": None}
    raise SystemExit(f"unknown method {method}")


def watch(data):
    """Like the app, it also repeats the goal every so often while it runs (an elapsed-time tick)."""
    path = _path(data, "events.jsonl")
    offset, tick = 0, time.monotonic()
    while True:
        goal = _load(data, "goal.json", None)
        if (goal or {}).get("status") == "running" and time.monotonic() - tick >= 0.3:
            tick = time.monotonic()
            frame = {"name": "thread-event", "payload": {"thread_id": "t1", "run_id": "",
                                                         "event": {"kind": "goal_changed", "goal": goal}}}
            print(json.dumps(frame), flush=True)
        if os.path.exists(path):
            with open(path, "rb") as handle:
                handle.seek(offset)
                for line in handle:
                    if not line.endswith(b"\n"):
                        break
                    offset += len(line)
                    print(line.decode("utf-8"), end="", flush=True)
        time.sleep(0.05)


def main(argv):
    if argv[:1] != ["--data-dir"]:
        raise SystemExit("usage: fake_themis --data-dir DIR ...")
    data, words = argv[1], argv[2:]
    if words == ["server", "start"] or words == ["server", "stop"]:
        print("{}")
    elif words[:2] == ["thread", "create"]:
        _save(data, "state.json", {"project": words[2], "model": words[3]})
        print(json.dumps({"id": "t1"}))
    elif words[:1] == ["call"]:
        print(json.dumps(call(data, words[1], json.loads(words[2]) if len(words) > 2 else {})))
    elif words == ["watch"]:
        watch(data)
    elif words[:1] == ["agent"]:
        agent(data, words[1], words[2])
    else:
        raise SystemExit(f"unknown command {words}")


if __name__ == "__main__":
    main(sys.argv[1:])
