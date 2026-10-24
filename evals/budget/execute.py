"""Runs one arm (a generated project, three messages, one budget) through the tool-eval runner
or the real app, watching its spend while it runs, and grades what exists when it ends.

Both paths return the same outcome: tool calls with their summaries, checkpoint places, the
usage totals, how the run ended and the graded project. A run that passes its per-run cost
limit is stopped (the app's chat is stopped; the runner is killed) and graded as it stands,
marked `stopped: cost`.
"""

import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time

from evals.hard.app import _cli, keep_evidence
from evals.longform.__main__ import isolated_env, sandbox_wrapper
from evals.tools.tasks import commit_all, git

from . import spend
from .generate import generate, material_tokens

EVIDENCE = ".themis/context/eval-thread"
TAIL = 4096


def _project(workdir, arm):
    project = os.path.join(workdir, "project")
    os.makedirs(project)
    spec = generate(arm["size"], arm["seed"], project)
    material = material_tokens(project)
    git(project, "init", "-q")
    commit_all(project, "Scratch project")
    return project, spec, material


def _kill(process):
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except (OSError, AttributeError):
        process.kill()


def run_runner(runner, arm, messages_of, grade_of, meter, limit, options):
    """One arm through the tool-eval runner. `messages_of(spec)` gives the chat's messages,
    `grade_of(project, spec)` grades the project, `limit(meter)` says when to stop for cost."""
    workdir = tempfile.mkdtemp(prefix="budget-run-")
    started = time.monotonic()
    try:
        project, spec, material = _project(workdir, arm)
        prompts = messages_of(spec)
        task_file = os.path.join(workdir, "task.json")
        with open(task_file, "w", encoding="utf-8") as handle:
            json.dump({"prompt": prompts[0], "followups": prompts[1:], "budget": arm["budget"],
                       "recent_messages": 20, "allow_all": True, "evidence": EVIDENCE}, handle)
        command = runner + ["--project", project, "--task", task_file, "--max-turns", str(options["max_turns"])]
        if options.get("model"):
            command += ["--model", options["model"]]
        if options.get("base_url"):
            command += ["--base-url", options["base_url"]]
        command = sandbox_wrapper(runner, workdir, options.get("sandbox", "auto")) + command
        out_path = os.path.join(workdir, "report.json")
        state = {"stopped": None, "stderr": ""}
        with open(out_path, "w", encoding="utf-8") as out:
            process = subprocess.Popen(command, stdout=out, stderr=subprocess.PIPE, text=True, cwd=workdir,
                                       env=isolated_env(workdir), start_new_session=True)

            def watch():
                for line in process.stderr:
                    if line.startswith("eval-usage "):
                        try:
                            counts = json.loads(line[len("eval-usage "):])
                        except ValueError:
                            continue
                        meter.update(counts.get("turn", 0), counts)
                        if state["stopped"] is None and limit(meter):
                            state["stopped"] = "cost"
                            _kill(process)
                    else:
                        state["stderr"] = (state["stderr"] + line)[-TAIL:]
            reader = threading.Thread(target=watch, daemon=True)
            reader.start()
            try:
                process.wait(timeout=options["timeout"])
            except subprocess.TimeoutExpired:
                state["stopped"] = state["stopped"] or "timeout"
                _kill(process)
                process.wait()
            reader.join(timeout=10)
            process.stderr.close()
        report = None
        if process.returncode == 0:
            try:
                with open(out_path, encoding="utf-8") as handle:
                    report = json.load(handle)
            except (OSError, ValueError):
                report = None
        outcome = _from_report(report, meter, state, process.returncode)
        outcome.update(grade=grade_of(project, spec), material_tokens=material,
                       wall_s=round(time.monotonic() - started, 1))
        return outcome
    finally:
        if options.get("keep"):
            keep_evidence(workdir, options["keep"], arm["name"])
        shutil.rmtree(workdir, ignore_errors=True)


def _from_report(report, meter, state, returncode):
    if report is None:
        error = state["stderr"].strip()[-300:] or f"runner exit {returncode}"
        # Killed for cost or time: the live usage is what is known; tool calls are not.
        return {"calls": None, "checkpoints": None, "turn_calls": None, "answer": "", "error": error,
                "finished": False, "stopped": state["stopped"], "usage": meter.usage(),
                "compactions": None}
    usage = meter.usage()
    usage.update({"requests": report.get("requests") or usage["requests"],
                  "input_tokens": report.get("input_tokens") or usage["input_tokens"],
                  "output_tokens": report.get("output_tokens") or usage["output_tokens"],
                  "cached_input_tokens": report.get("cached_input_tokens", usage["cached_input_tokens"]),
                  "reasoning_output_tokens": report.get("reasoning_output_tokens", usage["reasoning_output_tokens"]),
                  "estimated_input_tokens": report.get("estimated_input_tokens", usage["estimated_input_tokens"])})
    return {"calls": [{"tool": c.get("tool"), "summary": c.get("summary", ""), "ok": c.get("ok")}
                      for c in report.get("calls", [])],
            "checkpoints": [{"calls_before": c.get("calls_before", 0)} for c in report.get("checkpoints", [])],
            "turn_calls": [t.get("calls", 0) for t in report.get("turns", [])],
            "answer": (report.get("answer") or "")[:300], "error": report.get("error"),
            "finished": bool(report.get("finished")), "stopped": state["stopped"], "usage": usage,
            "compactions": report.get("compactions", 0)}


def run_app(binary, arm, messages_of, grade_of, meter, limit, options):
    """One arm as a real app chat: the `themis` CLI and app server with their own data folder and
    HOME, YOLO approvals, and the arm's budget in the data folder's runtime.jsonc."""
    workdir = tempfile.mkdtemp(prefix="budget-app-")
    started = time.monotonic()
    data, home = os.path.join(workdir, "data"), os.path.join(workdir, "home")
    os.makedirs(data)
    os.makedirs(home)
    env = {**os.environ, "HOME": home}
    try:
        project, spec, material = _project(workdir, arm)
        with open(os.path.join(data, "runtime.jsonc"), "w", encoding="utf-8") as handle:
            json.dump({"schema_version": 1, "context_token_budget": arm["budget"]}, handle)
        boot = _cli(binary, data, env, "server", "start")
        if boot.returncode != 0:
            return {"infrastructure": f"server start: {boot.stderr[-200:]}"}
        created = _cli(binary, data, env, "thread", "create", project, options["model"])
        try:
            thread = json.loads(created.stdout)["id"]
        except (ValueError, KeyError):
            return {"infrastructure": f"thread create: {(created.stderr or created.stdout)[-200:]}"}
        mode = _cli(binary, data, env, "call", "set_thread_approval_mode",
                    json.dumps({"threadId": thread, "mode": "yolo"}))
        if mode.returncode != 0:
            return {"infrastructure": f"approval mode: {mode.stderr[-200:]}"}
        deadline = started + options["timeout"]
        state = {"stopped": None}
        turns = []
        for index, prompt in enumerate(messages_of(spec)):
            turns.append(_app_turn(binary, data, env, thread, prompt, index, meter, limit, state, deadline))
            if state["stopped"]:
                break
        outcome = _from_events(turns, meter, state)
        outcome.update(grade=grade_of(project, spec), material_tokens=material,
                       wall_s=round(time.monotonic() - started, 1))
        return outcome
    finally:
        _cli(binary, data, env, "server", "stop")
        if options.get("keep"):
            keep_evidence(workdir, options["keep"], arm["name"])
        shutil.rmtree(workdir, ignore_errors=True)


def _app_turn(binary, data, env, thread, prompt, index, meter, limit, state, deadline):
    """One message's events, stopping the chat when the run passes its cost or time limit."""
    events = []
    process = subprocess.Popen([binary, "--data-dir", data, "thread", "send", thread, prompt, "--json"],
                               stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, env=env,
                               start_new_session=True)

    def stop(reason):
        if state["stopped"] is None:
            state["stopped"] = reason
            _cli(binary, data, env, "thread", "stop", thread)

    def read():
        for line in process.stdout:
            try:
                event = json.loads(line)["payload"]["event"]
            except (ValueError, KeyError, TypeError):
                continue
            events.append(event)
            if event.get("kind") == "context_usage":
                meter.update(index, spend.metrics_counts(event.get("metrics") or {}))
                if limit(meter):
                    stop("cost")
    reader = threading.Thread(target=read, daemon=True)
    reader.start()
    try:
        process.wait(timeout=max(1, deadline - time.monotonic()))
    except subprocess.TimeoutExpired:
        stop("timeout")
        try:
            process.wait(timeout=60)
        except subprocess.TimeoutExpired:
            _kill(process)
            process.wait()
    reader.join(timeout=10)
    process.stdout.close()
    return events


def _from_events(turns, meter, state):
    calls, checkpoints, turn_calls, compactions = [], [], [], 0
    for turn in turns:
        turn_calls.append(sum(e.get("kind") == "tool_started" for e in turn))
        for event in turn:
            kind = event.get("kind")
            if kind == "tool_started":
                calls.append({"tool": event.get("tool"), "summary": event.get("summary", ""), "ok": None})
            elif kind == "context_checkpoint":
                checkpoints.append({"calls_before": len(calls)})
            elif kind == "context_compacting":
                compactions += 1
    events = [e for turn in turns for e in turn]
    end = next((e for e in reversed(events) if e.get("kind") in ("finished", "failed", "incomplete")), None)
    error = None
    if end is None:
        error = "no final event"
    elif end["kind"] == "failed":
        error = end.get("error") or "failed"
    elif end["kind"] == "incomplete":
        error = "incomplete: " + (end.get("result") or "")[:200]
    finished = end is not None and end["kind"] == "finished" and len(turns) == 3 and not state["stopped"]
    return {"calls": calls, "checkpoints": checkpoints, "turn_calls": turn_calls,
            "answer": ((end or {}).get("result") or "")[:300], "error": error, "finished": finished,
            "stopped": state["stopped"], "usage": meter.usage(), "compactions": compactions}


def default_runner(root):
    """The tool-eval runner (debug build) from this checkout, built by the caller."""
    target = os.environ.get("CARGO_TARGET_DIR") or os.path.join(root, "target")
    return [os.path.join(target, "debug", "examples", "tool_eval" + (".exe" if os.name == "nt" else ""))]


def build_runner(root):
    subprocess.run(["cargo", "build", "-q", "-p", "themis-core", "--features", "eval", "--example", "tool_eval"],
                   cwd=root, check=True)
    return default_runner(root)


def build_app(root):
    built = subprocess.run(["cargo", "build", "-q", "-p", "themis-desktop", "--bin", "themis"], cwd=root, check=False)
    if built.returncode != 0:
        sys.exit(built.returncode)
    target = os.environ.get("CARGO_TARGET_DIR") or os.path.join(root, "target")
    return os.path.join(target, "debug", "themis")
