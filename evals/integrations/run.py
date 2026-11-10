"""Runs one integrations scenario through the real app with a simulated person, and records its trace.

A scratch HOME and data folder (no Keychain, no real app data), a small Git project holding the
scenario's files, Custom mode where every change asks, and the `themis` CLI: the person's own
saves first (the scenario's `seed`), then a chat, the scenario's message, and events from
`themis watch`. The simulated person allows every approval once and answers every question with
"your call"; a turn that ends with the hidden check not met gets one nudge ("Yes, please go ahead
and do it."). The run ends when no turn runs and nothing waits, at the time limit, or as soon as
the provider answers 429 (rate limited), whose Retry-After is kept in the trace.
"""

import json
import os
import re
import shutil
import subprocess
import tempfile
import time

from evals.kit import app
from evals.kit.env import isolated_env
from evals.questions.run import ApprovalWatch, active_run, command_text

from . import scenarios

NUDGE = "Yes, please go ahead and do it."
MAX_NUDGES = 1
APPROVAL = {"default": "ask", "rules": [{"tool": "shell_unsandboxed", "action": "deny"},
                                        {"tool": "*", "risk": "read", "action": "allow"}]}
RATE_LIMITED = re.compile(r"\b429\b|rate.?limit", re.I)
RETRY_AFTER = re.compile(r"retry.?after\D{0,5}(\d+)", re.I)


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)


class IntegrationsRun:
    def __init__(self, binary, options, scenario, seed):
        self.binary, self.options, self.scenario, self.seed = binary, options, scenario, seed
        self.actions = []

    def cli(self, *args, timeout=120):
        return app.cli(self.binary, self.data, self.env, *args, timeout=timeout)

    def call(self, method, args=None, timeout=60):
        return app.call(self.binary, self.data, self.env, method, args, timeout=timeout)

    def store(self):
        """Every plugin the person's own CLI lists (global and the project's)."""
        done = self.cli("call", "plugin_action", json.dumps({"action": "list", "projectRoot": self.project}))
        try:
            return json.loads(done.stdout)
        except ValueError:
            return None

    def run(self):
        started = time.monotonic()
        self.workdir = tempfile.mkdtemp(prefix="integrations-")
        self.data = os.path.join(self.workdir, "data")
        self.project = os.path.join(os.path.realpath(self.workdir), "project")
        os.makedirs(self.data)
        self._project()
        self.env = isolated_env(self.workdir)
        with open(os.path.join(self.data, "runtime.jsonc"), "w", encoding="utf-8") as handle:
            json.dump({"schema_version": 1, "approval": APPROVAL}, handle)
        trace = {"scenario": self.scenario["name"], "seed": self.seed, "model": self.options["model"],
                 "events": [], "approvals": [], "actions": self.actions, "end": None, "store": None,
                 "store_before": None, "subagent_transcripts": []}
        self.watch = None
        try:
            trace["end"] = self._drive(started, trace)
        except Exception as error:  # noqa: BLE001 - a runner error must still leave a trace
            trace["end"] = {"reason": "crash", "detail": f"runner: {type(error).__name__}: {str(error)[:200]}"}
        finally:
            if self.watch is not None:
                trace["events"] = self.watch.snapshot()
                trace["approvals"] = [{"id": r.get("approval_id"), "tool": r.get("tool"), "command": command_text(r)}
                                      for r in self.watch.approvals]
            trace["store"] = self.store()
            trace["project_changes"] = self._project_changes()
            trace["subagent_transcripts"] = self._subagent_transcripts()
            self.cli("server", "stop")
            if self.watch is not None:
                self.watch.stop()
            trace["wall_s"] = round(time.monotonic() - started, 1)
            from .grade import evaluate

            evaluate(trace, self.scenario, self.project)
            if self.options.get("keep"):
                app.keep_evidence(self.workdir, self.options["keep"], f"{self.scenario['name']}-{self.seed}")
            shutil.rmtree(self.workdir, ignore_errors=True)
        return trace

    def _project(self):
        os.makedirs(self.project)
        write(os.path.join(self.project, "README.md"), "# Notes app\n\nA small project for the integrations eval.\n")
        for path, text in scenarios.FILES.items():
            write(os.path.join(self.project, path), text)
        for argv in (["git", "init", "-q"], ["git", "add", "-A"],
                     ["git", "-c", "user.email=e@x", "-c", "user.name=e", "commit", "-qm", "start"],
                     ["git", "-c", "user.email=e@x", "-c", "user.name=e", "tag", "v0.1.0"]):
            subprocess.run(argv, cwd=self.project, check=True, capture_output=True)

    def _project_changes(self):
        """Paths the run added or changed in the project (Git's view, untracked files listed one by one)."""
        done = subprocess.run(["git", "status", "--porcelain", "--untracked-files=all"], cwd=self.project,
                              capture_output=True, text=True)
        return [line[3:].strip('"') for line in done.stdout.splitlines() if len(line) > 3]

    def _seed(self):
        """The person's own saves before the chat, through their CLI (no agent credential)."""
        for index, item in enumerate(self.scenario.get("seed") or []):
            for name, definition in (item.get("mcp") or {}).items():
                text = json.dumps(definition).replace("{project}", self.project)
                path = os.path.join(self.workdir, f"seed-{index}-{name}.json")
                write(path, text)
                done = self.cli("mcp", "add", item["plugin"], name, path)
                if done.returncode != 0:
                    return f"seed mcp {name}: {(done.stderr or done.stdout)[-200:]}"
                self.cli("mcp", "enable", item["plugin"], name)
            for hook in item.get("hooks") or []:
                path = os.path.join(self.workdir, f"seed-{index}-{hook['name']}.json")
                write(path, json.dumps({k: v for k, v in hook.items() if k != "name"}))
                done = self.cli("hook", "add", item["plugin"], hook["name"], path)
                if done.returncode != 0:
                    return f"seed hook {hook['name']}: {(done.stderr or done.stdout)[-200:]}"
        return None

    def _drive(self, started, trace):
        boot = self.cli("server", "start", timeout=120)
        if boot.returncode != 0:
            return {"reason": "infrastructure", "detail": f"server start: {boot.stderr.strip()[-200:]}"}
        failed = self._seed()
        if failed:
            return {"reason": "infrastructure", "detail": failed}
        trace["store_before"] = self.store()
        created = self.cli("thread", "create", self.project, self.options["model"])
        try:
            self.thread = json.loads(created.stdout)["id"]
        except (ValueError, KeyError, TypeError):
            return {"reason": "infrastructure", "detail": f"thread create: {(created.stderr or created.stdout)[-200:]}"}
        _value, error = self.call("set_thread_approval_mode", {"threadId": self.thread, "mode": "custom"})
        if error:
            return {"reason": "infrastructure", "detail": f"approval mode: {error}"}
        self.watch = ApprovalWatch(self.binary, self.data, self.env, self.thread)
        time.sleep(2)
        _handle, error = self.call("send_message", {"threadId": self.thread, "text": self.scenario["message"]})
        if error:
            return {"reason": "infrastructure", "detail": f"send: {error}"}
        seen, answered, nudges, idle_since = set(), set(), 0, None
        deadline = started + self.options["timeout"]
        while time.monotonic() < deadline:
            self._answer_approvals(seen)
            self._answer_questions(answered)
            events = self.watch.snapshot()
            limited = rate_limit(events)
            if limited:
                self.call("stop_thread", {"threadId": self.thread}, timeout=30)
                return {"reason": "rate_limited", "detail": limited}
            if active_run(events) or not any(e["event"].get("kind") == "started" for e in events):
                idle_since = None
                time.sleep(1)
                continue
            idle_since = idle_since or time.monotonic()
            if time.monotonic() - idle_since < 6:
                time.sleep(1)
                continue
            if self._open_questions():
                continue
            from .grade import hidden_check

            if (nudges < MAX_NUDGES and self.scenario["check"]["kind"] != "diagnosed"
                    and not hidden_check(self.scenario, self.store(), self.project)["met"]):
                nudges += 1
                self.actions.append({"kind": "nudge", "t": round(time.monotonic() - started, 1)})
                self.call("send_message", {"threadId": self.thread, "text": NUDGE})
                idle_since = None
                continue
            return {"reason": "finished", "detail": ""}
        self.call("stop_thread", {"threadId": self.thread}, timeout=30)
        return {"reason": "cap_time", "detail": ""}

    def _answer_approvals(self, seen):
        for request in self.watch.pending_approvals(seen):
            seen.add(request.get("approval_id"))
            self.call("approve_action", {"threadId": self.thread, "approvalId": request.get("approval_id"),
                                         "decision": "once"})
            self.actions.append({"kind": "approval_once", "command": command_text(request)[:200]})

    def _open_questions(self):
        questions, _error = self.call("list_questions", {"threadId": self.thread}, timeout=30)
        return [q for q in questions or [] if q.get("answer") is None]

    def _answer_questions(self, answered):
        for question in self._open_questions():
            if question.get("id") in answered:
                continue
            answered.add(question["id"])
            self.call("answer_question", {"threadId": self.thread, "questionId": question["id"],
                                          "answer": "Your call; do what you think is best."})
            self.actions.append({"kind": "answer", "question": (question.get("question") or "")[:200]})

    def _subagent_transcripts(self):
        """The text of every subagent transcript the chat left in the data folder."""
        found = []
        for folder, _dirs, files in os.walk(self.data):
            if "subagent" not in folder:
                continue
            for name in files:
                try:
                    with open(os.path.join(folder, name), encoding="utf-8", errors="replace") as handle:
                        found.append(handle.read()[:400_000])
                except OSError:
                    continue
        return found


def rate_limit(events):
    """The Retry-After (or the failure text) of a run that failed on a 429, else None."""
    for item in events:
        event = item["event"]
        if event.get("kind") == "failed":
            text = json.dumps(event)
            if RATE_LIMITED.search(text):
                retry = RETRY_AFTER.search(text)
                return f"429, retry after {retry.group(1)}s" if retry else f"429: {text[:200]}"
    return None
