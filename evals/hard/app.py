"""Runs a task through the real app: the `themis` CLI and app server, built from this checkout.

Each run gets its own data folder and HOME (so no Keychain and no real app data are touched; the
key comes from `OPENCODE_KEY`), a scratch git project, and a chat set to YOLO approvals. The app's
own OS sandbox applies to the agent's commands, as in the desktop app. Squeezed memory is set
through the data folder's `runtime.jsonc`.
"""

import json
import os
import shutil
import subprocess
import tempfile
import time

from evals.tools.tasks import commit_all, git


def themis_binary(root):
    """The debug `themis` CLI from this checkout (built by the caller)."""
    return os.path.join(root, "target", "debug", "themis")


def _cli(binary, data, env, *args, stdin=None, timeout=120):
    return subprocess.run([binary, "--data-dir", data, *args], input=stdin, capture_output=True,
                          text=True, env=env, timeout=timeout, check=False)


def run_app_task(binary, task, model, timeout=5400, keep=None):
    """(row, error): one task as a real app chat, graded by the task's check."""
    workdir = tempfile.mkdtemp(prefix="hard-app-")
    data, home, project = (os.path.join(workdir, name) for name in ("data", "home", "project"))
    for path in (data, home, project):
        os.makedirs(path)
    env = {**os.environ, "HOME": home}
    if task["budget"] != 200_000:
        with open(os.path.join(data, "runtime.jsonc"), "w", encoding="utf-8") as handle:
            json.dump({"schema_version": 1, "context_token_budget": task["budget"],
                       "recent_messages": task["recent_messages"]}, handle)
    git(project, "init", "-q")
    task["setup"](project)
    with open(os.path.join(project, ".gitkeep"), "w", encoding="utf-8"):
        pass
    commit_all(project, "Scratch project")
    started = time.monotonic()
    try:
        boot = _cli(binary, data, env, "server", "start")
        if boot.returncode != 0:
            return None, f"server start: {boot.stderr[-200:]}"
        created = _cli(binary, data, env, "thread", "create", project, model)
        try:
            thread = json.loads(created.stdout)["id"]
        except (ValueError, KeyError):
            return None, f"thread create: {(created.stderr or created.stdout)[-200:]}"
        mode = _cli(binary, data, env, "call", "set_thread_approval_mode",
                    json.dumps({"threadId": thread, "mode": "yolo"}))
        if mode.returncode != 0:
            return None, f"approval mode: {mode.stderr[-200:]}"
        try:
            sent = _cli(binary, data, env, "thread", "send", thread, task["prompt"], "--json", timeout=timeout)
        except subprocess.TimeoutExpired:
            return None, "timeout"
        events = []
        for line in sent.stdout.splitlines():
            try:
                events.append(json.loads(line)["payload"]["event"])
            except (ValueError, KeyError, TypeError):
                continue
        kinds = [event.get("kind") for event in events]
        end = next((e for e in reversed(events) if e.get("kind") in ("finished", "failed", "incomplete")), None)
        if end is None:
            return None, f"no final event: {sent.stderr[-200:]}"
        error = end.get("error") if end["kind"] != "finished" else None
        if error and error.startswith("LLM request failed") and kinds.count("tool_started") == 0:
            return None, f"provider: {error[:160]}"
        passed, why = task["check"](project, {"answer": end.get("result", "")})
        row = {"task": task["name"], "tool": task["tool"], "passed": passed and end["kind"] == "finished",
               "why": why if end["kind"] == "finished" else f"{end['kind']}: {(error or end.get('result', ''))[:160]}; {why}",
               "answer": (end.get("result") or "")[:300], "error_text": (error or "")[:300],
               "calls": [{"tool": e.get("tool")} for e in events if e.get("kind") == "tool_started"],
               "compactions": kinds.count("context_compacting"),
               "latency_ms": int((time.monotonic() - started) * 1000), "via": "app"}
        return row, None
    finally:
        _cli(binary, data, env, "server", "stop")
        if keep:
            os.makedirs(keep, exist_ok=True)
            shutil.copytree(workdir, tempfile.mkdtemp(prefix=f"{task['name']}-", dir=keep), dirs_exist_ok=True,
                            ignore=shutil.ignore_patterns("vendor", "__pycache__", ".git"))
        shutil.rmtree(workdir, ignore_errors=True)
