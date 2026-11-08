"""Runs one questions scenario through the real app with a simulated person, and records its trace.

A scratch HOME and data folder (no Keychain, no real app data), a generated Ledgerly project (the
verify suite's reference project plus the scenario's extra files), Custom mode where every
change asks, and the `themis` CLI: a chat, the scenario's message, events from `themis watch`.
The simulated person (`Person`) answers each approval and question as it arrives:

- approvals: Deny those the scenario refuses (`refuse`: patterns in the command), allow the rest
  once, so nothing else waits;
- questions (`ask_user`): each open question is answered from the scenario's hidden
  `preferences` (a choice that names the preference, else the preference in words);
- a turn that ends asking in plain words, with the work not done, gets the scenario's `nudge` as
  a chat message, at most `MAX_NUDGES` times.

The run ends when no turn runs and nothing waits, or at the time limit.
"""

import json
import os
import re
import shutil
import subprocess
import tempfile
import time

from evals.kit import app
from evals.kit.app import TERMINAL, Watch
from evals.kit.env import isolated_env
from evals.verify import generate

MAX_NUDGES = 2
APPROVAL = {"default": "ask", "rules": [{"tool": "shell_unsandboxed", "action": "deny"},
                                        {"tool": "*", "risk": "read", "action": "allow"}]}


class ApprovalWatch(Watch):
    """`themis watch` that also keeps the chat's `approval-request` frames."""

    def __init__(self, *args, **kwargs):
        self.approvals = []
        super().__init__(*args, **kwargs)

    def _read(self):
        for line in self.process.stdout:
            try:
                frame = json.loads(line)
            except ValueError:
                continue
            payload = frame.get("payload") or {}
            if frame.get("name") == "approval-request" and payload.get("thread_id") == self.thread:
                with self.lock:
                    self.approvals.append(payload)
                continue
            if frame.get("name") != "thread-event" or payload.get("thread_id") != self.thread:
                continue
            if not isinstance(payload.get("event"), dict):
                continue
            item = {"t": round(time.monotonic() - self.started, 2), "run_id": payload.get("run_id") or "",
                    "event": payload["event"]}
            with self.lock:
                self.events.append(item)
                self.last = time.monotonic()

    def pending_approvals(self, seen):
        with self.lock:
            return [request for request in self.approvals if request.get("approval_id") not in seen]


def command_text(request):
    command = request.get("command") or {}
    argv = command.get("argv") if isinstance(command, dict) else None
    return " ".join(argv) if argv else str(request.get("summary") or "")


def answer_for(question, preferences):
    """The person's answer to one question: a choice naming the matching preference, else the
    preference in words; a question matching several preferences gets them all in words."""
    text = (question.get("question") or "").lower()
    matched = [pref for pref in preferences if any(word in text for word in pref["about"])]
    if not matched:
        return "Your call; pick what you think is best.", None
    if len(matched) > 1:
        return " ".join(pref["say"] for pref in matched), None
    pref = matched[0]
    for choice in question.get("choices") or []:
        label = choice if isinstance(choice, str) else (choice.get("answer") or choice.get("label") or "")
        if any(word in label.lower() for word in pref["pick"]):
            return label, pref["id"]
    return pref["say"], pref["id"]


class QuestionsRun:
    def __init__(self, binary, options, scenario, seed):
        self.binary, self.options, self.scenario, self.seed = binary, options, scenario, seed
        self.actions = []  # what the person did, in order

    def call(self, method, args=None, timeout=60):
        return app.call(self.binary, self.data, self.env, method, args, timeout=timeout)

    def run(self):
        started = time.monotonic()
        self.workdir = tempfile.mkdtemp(prefix="questions-")
        self.data = os.path.join(self.workdir, "data")
        self.project = os.path.join(self.workdir, "project")
        os.makedirs(self.data)
        generate.project(self.project, None, self.seed, git=True)
        self._add_files()
        self.env = isolated_env(self.workdir)
        with open(os.path.join(self.data, "runtime.jsonc"), "w", encoding="utf-8") as handle:
            json.dump({"schema_version": 1, "approval": APPROVAL}, handle)
        trace = {"scenario": self.scenario["name"], "seed": self.seed, "model": self.options["model"],
                 "events": [], "approvals": [], "actions": self.actions, "end": None}
        self.watch = None
        try:
            trace["end"] = self._drive(started)
        except Exception as error:  # noqa: BLE001 - a runner error must still leave a trace
            trace["end"] = {"reason": "crash", "detail": f"runner: {type(error).__name__}: {str(error)[:200]}"}
        finally:
            if self.watch is not None:
                trace["events"] = self.watch.snapshot()
                trace["approvals"] = [{"id": r.get("approval_id"), "tool": r.get("tool"), "command": command_text(r)}
                                      for r in self.watch.approvals]
            app.cli(self.binary, self.data, self.env, "server", "stop")
            if self.watch is not None:
                self.watch.stop()
            trace["wall_s"] = round(time.monotonic() - started, 1)
            from .grade import evaluate

            evaluate(trace, self.scenario, self.project)
            shutil.rmtree(self.workdir, ignore_errors=True)
        return trace

    def _add_files(self):
        for path, text in (self.scenario.get("files") or {}).items():
            target = os.path.join(self.project, path)
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with open(target, "w", encoding="utf-8") as handle:
                handle.write(text)
        if self.scenario.get("files"):
            subprocess.run(["git", "add", "-A"], cwd=self.project, check=True, capture_output=True)
            subprocess.run(["git", "-c", "user.email=e@x", "-c", "user.name=e", "commit", "-qm", "scenario files"],
                           cwd=self.project, check=True, capture_output=True)

    def _drive(self, started):
        boot = app.cli(self.binary, self.data, self.env, "server", "start", timeout=120)
        if boot.returncode != 0:
            return {"reason": "infrastructure", "detail": f"server start: {boot.stderr.strip()[-200:]}"}
        created = app.cli(self.binary, self.data, self.env, "thread", "create", self.project, self.options["model"])
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
        seen_approvals, answered, nudges, idle_since = set(), set(), 0, None
        deadline = started + self.options["timeout"]
        while time.monotonic() < deadline:
            self._answer_approvals(seen_approvals)
            self._answer_questions(answered)
            events = self.watch.snapshot()
            running = active_run(events)
            if running or not any(e["event"].get("kind") == "started" for e in events):
                idle_since = None
                time.sleep(1)
                continue
            idle_since = idle_since or time.monotonic()
            if time.monotonic() - idle_since < 6:
                time.sleep(1)
                continue
            if self._open_questions():
                continue
            if nudges < MAX_NUDGES and not done(self.scenario, self.project):
                nudges += 1
                self.actions.append({"kind": "nudge", "t": round(time.monotonic() - started, 1)})
                self.call("send_message", {"threadId": self.thread, "text": self.scenario["nudge"]})
                idle_since = None
                continue
            return {"reason": "finished", "detail": ""}
        self.call("stop_thread", {"threadId": self.thread}, timeout=30)
        return {"reason": "cap_time", "detail": ""}

    def _answer_approvals(self, seen):
        for request in self.watch.pending_approvals(seen):
            seen.add(request.get("approval_id"))
            text = command_text(request).lower()
            refuse = any(re.search(pattern, text) for pattern in self.scenario.get("refuse") or [])
            decision = "deny" if refuse else "once"
            self.call("approve_action", {"threadId": self.thread, "approvalId": request.get("approval_id"),
                                         "decision": decision})
            self.actions.append({"kind": f"approval_{decision}", "command": text[:200]})

    def _open_questions(self):
        questions, _error = self.call("list_questions", {"threadId": self.thread}, timeout=30)
        return [q for q in questions or [] if q.get("answer") is None]

    def _answer_questions(self, answered):
        """Answers each open question; the pages of one set (`group`) together in one reply, as the
        box sends them."""
        sets = {}
        for question in self._open_questions():
            if question.get("id") not in answered:
                sets.setdefault(question.get("group") or question["id"], []).append(question)
        for pages in sets.values():
            replies = []
            for question in pages:
                answered.add(question["id"])
                text, pref = answer_for(question, self.scenario.get("preferences") or [])
                replies.append({"questionId": question["id"], "answer": text})
                self.actions.append({"kind": "answer", "question": (question.get("question") or "")[:200],
                                     "choices": len(question.get("choices") or []), "matched": pref,
                                     "answer": text, "pages": len(pages)})
            if len(replies) == 1:
                self.call("answer_question", {"threadId": self.thread, **replies[0]})
            else:
                self.call("answer_question", {"threadId": self.thread, "answers": replies})
            self.actions.append({"kind": "reply", "pages": len(replies)})


def active_run(events):
    open_runs = set()
    for item in events:
        kind = item["event"].get("kind")
        if kind == "started":
            open_runs.add(item["run_id"])
        elif kind in TERMINAL:
            open_runs.discard(item["run_id"])
    return bool(open_runs)


def done(scenario, project):
    """Whether the scenario's hidden check already passes (so no nudge is needed)."""
    from .grade import hidden_score

    return hidden_score(scenario, project)["met"] >= 1

