"""Runs one soak through the real app, exactly as a person's goal runs, and records its trace.

A scratch HOME and data folder (no Keychain, no real app data), a generated git project, the
budget in the data folder's `runtime.jsonc`, YOLO approvals, then through the `themis` CLI:
the rules as an ordinary first message, `/goal ...` with `send_message`, events from
`themis watch`, the goal's status from `get_goal`, the follow-up queued with `enqueue_message`
once half the items are written, the recall questions after `reached`, and the saved
checkpoints from `get_thread_history_page`.

Spend is metered live from `context_usage` events and the app's usage ledger
(`get_usage_summary`); a run that passes its cost cap, turn cap or time limit is paused and
stopped at once and recorded with that end.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time

from evals.kit import app, git
from evals.kit import spend as spend_kit
from evals.kit.env import isolated_env

from .generate import generate, material_tokens, messages

CHECKER = os.path.join(os.path.dirname(os.path.abspath(__file__)), "check_template.py")
TERMINAL = ("finished", "failed", "incomplete")
STOPPED_GOAL = {"reached": "reached", "stuck": "stuck", "waiting": "waiting", "quota": "infrastructure",
                "paused": "failed"}


class Watch:
    """`themis watch` for one chat: every thread event in arrival order, with its time."""

    def __init__(self, binary, data, env, thread, on_event=None):
        self.started = time.monotonic()
        self.events, self.thread, self.on_event = [], thread, on_event
        self.last = time.monotonic()
        self.lock = threading.Lock()
        self.process = subprocess.Popen([binary, "--data-dir", data, "watch"], stdout=subprocess.PIPE,
                                        stderr=subprocess.DEVNULL, text=True, env=env, start_new_session=True)
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self):
        for line in self.process.stdout:
            try:
                frame = json.loads(line)
            except ValueError:
                continue
            if frame.get("name") != "thread-event":
                continue
            payload = frame.get("payload") or {}
            if payload.get("thread_id") != self.thread or not isinstance(payload.get("event"), dict):
                continue
            item = {"t": round(time.monotonic() - self.started, 2), "run_id": payload.get("run_id") or "",
                    "event": payload["event"]}
            with self.lock:
                self.events.append(item)
                self.last = time.monotonic()
            if self.on_event:
                self.on_event(item)

    def alive(self):
        return self.process.poll() is None

    def snapshot(self):
        with self.lock:
            return list(self.events)

    def run_end(self, run_id):
        with self.lock:
            return next((item["event"] for item in self.events
                         if item["run_id"] == run_id and item["event"].get("kind") in TERMINAL), None)

    def stop(self):
        app.kill(self.process)
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            pass
        self.reader.join(timeout=5)


class Soak:
    """One run: `options` holds model, items, filler_chars, max_turns, timeout, idle_timeout,
    poll_s, max_run_cost, keep."""

    def __init__(self, binary, options, budget, seed, price):
        self.binary, self.options, self.budget, self.seed = binary, options, budget, seed
        self.meter = spend_kit.LedgerMeter(price) if price else None
        self.stop_reason = None
        self._stop_lock = threading.Lock()
        self.calls_failing = 0

    # -- plumbing ------------------------------------------------------------------------------

    def call(self, method, args=None, timeout=60):
        value, error = app.call(self.binary, self.data, self.env, method, args, timeout=timeout)
        if error and "no answer" in error:
            self.calls_failing += 1
        elif not error:
            self.calls_failing = 0
        return value, error

    def over_cost(self):
        return (self.meter is not None and not self.meter.price.free
                and self.meter.cost() > self.options["max_run_cost"])

    def halt(self, reason):
        """Pauses the goal and stops the chat once, recording why."""
        with self._stop_lock:
            if self.stop_reason is not None:
                return
            self.stop_reason = reason
        self.call("pause_goal", {"threadId": self.thread}, timeout=30)
        self.call("stop_thread", {"threadId": self.thread}, timeout=30)

    def on_event(self, item):
        event = item["event"]
        if event.get("kind") == "context_usage" and self.meter is not None:
            self.meter.update(item["run_id"], spend_kit.metrics_counts(event.get("metrics") or {}))
            if self.over_cost():
                threading.Thread(target=self.halt, args=("cap_cost",), daemon=True).start()

    def wait_run(self, run_id, limit):
        deadline = time.monotonic() + limit
        while time.monotonic() < deadline:
            end = self.watch.run_end(run_id)
            if end is not None or self.stop_reason or not self.watch.alive():
                return end
            time.sleep(1)
        return None

    def out_files(self):
        out = os.path.join(self.project, "out")
        if not os.path.isdir(out):
            return 0
        return sum(name.endswith(".md") and name != "INDEX.md" for name in os.listdir(out))

    # -- the run -------------------------------------------------------------------------------

    def run(self):
        started = time.monotonic()
        self.workdir = tempfile.mkdtemp(prefix="soak-")
        self.data = os.path.join(self.workdir, "data")
        self.project = os.path.join(self.workdir, "project")
        os.makedirs(self.data)
        os.makedirs(self.project)
        self.env = isolated_env(self.workdir)
        spec = generate(self.seed, self.project, self.options["items"], self.options["filler_chars"])
        script = messages(spec)
        git.init(self.project)
        with open(os.path.join(self.data, "runtime.jsonc"), "w", encoding="utf-8") as handle:
            json.dump({"schema_version": 1, "context_token_budget": self.budget}, handle)
        trace = {"spec": spec, "budget": self.budget, "seed": self.seed, "model": self.options["model"],
                 "mode": "yolo", "material_tokens": material_tokens(self.project), "events": [],
                 "goal_polls": [], "saved_checkpoints": [], "goal_text": script["goal"][len("/goal"):].strip(),
                 "followup": None, "recall": [], "end": None, "final": {}}
        self.thread, self.watch = None, None
        try:
            trace["end"] = self._drive(trace, script, started)
        except Exception as error:  # noqa: BLE001 - a runner error must still leave a trace
            trace["end"] = {"reason": "crash", "detail": f"runner: {type(error).__name__}: {str(error)[:200]}"}
        finally:
            if self.watch is not None:
                trace["events"] = self.watch.snapshot()
            if self.thread:
                trace["saved_checkpoints"] = self.saved_checkpoints()
            if self.meter is not None:
                self._ledger()
                trace["usage"], trace["cost"] = self.meter.usage(), self.meter.cost()
            app.cli(self.binary, self.data, self.env, "server", "stop")
            if self.watch is not None:
                self.watch.stop()
            trace["final"] = final_state(self.project, self.env)
            trace["wall_s"] = round(time.monotonic() - started, 1)
            if self.options.get("keep"):
                app.keep_evidence(self.workdir, self.options["keep"], f"soak-{self.budget}-{self.seed}")
            shutil.rmtree(self.workdir, ignore_errors=True)
        return trace

    def _drive(self, trace, script, started):
        boot = app.cli(self.binary, self.data, self.env, "server", "start", timeout=120)
        if boot.returncode != 0:
            return {"reason": "infrastructure", "detail": f"server start: {boot.stderr.strip()[-200:]}"}
        created = app.cli(self.binary, self.data, self.env, "thread", "create", self.project, self.options["model"])
        try:
            self.thread = json.loads(created.stdout)["id"]
        except (ValueError, KeyError, TypeError):
            return {"reason": "infrastructure", "detail": f"thread create: {(created.stderr or created.stdout)[-200:]}"}
        _value, error = self.call("set_thread_approval_mode", {"threadId": self.thread, "mode": "yolo"})
        if error:
            return {"reason": "infrastructure", "detail": f"approval mode: {error}"}
        self.watch = Watch(self.binary, self.data, self.env, self.thread, self.on_event)
        time.sleep(self.options.get("subscribe_s", 2))  # let the subscription start before the first run
        deadline = started + self.options["timeout"]

        handle, error = self.call("send_message", {"threadId": self.thread, "text": script["rules"]})
        if error or not isinstance(handle, dict):
            return {"reason": "infrastructure", "detail": f"first message: {error}"}
        end = self.wait_run(handle["run_id"], max(1, deadline - time.monotonic()))
        if end is None:
            return self._ended(deadline, "first message never finished")
        handle, error = self.call("send_message", {"threadId": self.thread, "text": script["goal"]})
        if error or not isinstance(handle, dict):
            return {"reason": "crash", "detail": f"/goal: {error}"}

        reason = self._goal_loop(trace, script, deadline)
        if reason["reason"] == "reached":
            self._after_reached(trace, script, deadline)
        return reason

    def _ended(self, deadline, detail):
        if self.stop_reason:
            return {"reason": self.stop_reason, "detail": ""}
        if not self.watch.alive():
            return {"reason": "crash", "detail": "event stream closed"}
        if time.monotonic() >= deadline:
            return {"reason": "cap_time", "detail": ""}
        return {"reason": "server_hang", "detail": detail}

    def _ledger(self):
        summary, error = self.call("get_usage_summary", None, timeout=30)
        if not error and summary:
            self.meter.update_ledger(spend_kit.ledger_counts(summary))

    def _goal_loop(self, trace, script, deadline):
        polls = 0
        active = [item for item in trace["spec"]["items"] if item["status"] != "retired"]
        threshold = max(1, len(active) // 2)
        while True:
            if self.stop_reason:
                return {"reason": self.stop_reason, "detail": ""}
            if not self.watch.alive():
                return {"reason": "crash", "detail": "event stream closed"}
            if time.monotonic() >= deadline:
                self.halt("cap_time")
                continue
            if self.calls_failing >= 2:
                return {"reason": "server_hang", "detail": "the server stopped answering"}
            goal, error = self.call("get_goal", {"threadId": self.thread}, timeout=30)
            if not error:
                trace["goal_polls"].append({"t": round(time.monotonic() - self.watch.started, 2), "phase": "goal",
                                            "goal": goal})
            polls += 1
            if self.meter is not None and polls % 3 == 0:
                self._ledger()
                if self.over_cost():
                    self.halt("cap_cost")
                    continue
            if isinstance(goal, dict):
                if goal.get("turns", 0) > self.options["max_turns"]:
                    self.halt("cap_turns")
                    continue
                status = goal.get("status")
                if status in STOPPED_GOAL:
                    return {"reason": STOPPED_GOAL[status], "detail": (goal.get("reason") or "")[:200]}
                if status == "running" and self._idle_too_long():
                    return {"reason": "not_continuing", "detail": "the goal runs but no turn started"}
            if trace["followup"] is None and self.out_files() >= threshold:
                queued, error = self.call("enqueue_message", {"threadId": self.thread, "text": script["followup"]})
                trace["followup"] = {"t": round(time.monotonic() - self.watch.started, 2), "text": script["followup"],
                                     "error": error, "after_files": self.out_files()}
            time.sleep(self.options["poll_s"])

    def _idle_too_long(self):
        if time.monotonic() - self.watch.last < self.options["idle_timeout"]:
            return False
        thread, error = self.call("get_thread", {"threadId": self.thread}, timeout=30)
        return not error and isinstance(thread, dict) and not thread.get("running")

    def _after_reached(self, trace, script, deadline):
        """Waits out any run still going, watches for a goal turn that must not start, then asks
        the recall questions as ordinary messages."""
        settle = time.monotonic() + 120
        while time.monotonic() < settle:
            thread, error = self.call("get_thread", {"threadId": self.thread}, timeout=30)
            if error or not (isinstance(thread, dict) and thread.get("running")):
                break
            time.sleep(2)
        time.sleep(self.options.get("grace_s", 30))
        for question in script["recall"]:
            if self.stop_reason or time.monotonic() >= deadline:
                break
            handle, error = self.call("send_message", {"threadId": self.thread, "text": question["text"]})
            answer = {"id": question["id"], "expect": question["expect"], "run_id": None, "answer": "", "tools": 0,
                      "error": error}
            if not error and isinstance(handle, dict):
                answer["run_id"] = handle["run_id"]
                end = self.wait_run(handle["run_id"], min(600, max(1, deadline - time.monotonic())))
                if end is not None and end.get("kind") == "finished":
                    answer["answer"] = (end.get("result") or "")[:500]
                answer["tools"] = sum(item["run_id"] == handle["run_id"] and item["event"].get("kind") == "tool_started"
                                      for item in self.watch.snapshot())
            trace["recall"].append(answer)

    def saved_checkpoints(self):
        """Checkpoint summaries in the saved history, read page by page."""
        summaries, cursor, through, carry = [], None, None, ""
        for _page in range(10_000):
            args = {"threadId": self.thread, "maxBytes": 1 << 20}
            if cursor is not None:
                args["cursor"] = cursor
            if through is not None:
                args["throughSeq"] = through
            page, error = self.call("get_thread_history_page", args, timeout=60)
            if error or not isinstance(page, dict):
                break
            through = page.get("through_seq")
            lines = (carry + (page.get("jsonl") or "")).split("\n")
            carry = lines.pop()
            for line in lines:
                summary = checkpoint_summary(line)
                if summary is not None:
                    summaries.append(summary)
            cursor = page.get("next_cursor")
            if not cursor:
                break
        summary = checkpoint_summary(carry)
        if summary is not None:
            summaries.append(summary)
        return summaries


def checkpoint_summary(line):
    """The summary of a saved `context_checkpoint` event in one history line, else None."""
    try:
        item = json.loads(line)
    except ValueError:
        return None
    event = ((item or {}).get("envelope") or {}).get("event") or {}
    return event.get("summary") if item.get("kind") == "event" and event.get("kind") == "context_checkpoint" else None


def final_state(project, env):
    """The project's outputs, `check.py`'s verdict and the git state when the run ended."""
    files = {}
    out = os.path.join(project, "out")
    if os.path.isdir(out):
        for name in sorted(os.listdir(out)):
            path = os.path.join(out, name)
            if os.path.isfile(path):
                with open(path, encoding="utf-8", errors="replace") as handle:
                    files[f"out/{name}"] = handle.read(20_000)
    # The harness's own copy of the checker: the agent may have edited the project's check.py.
    checker = os.path.join(project, ".soak-check.py")
    shutil.copy(CHECKER, checker)
    try:
        done = subprocess.run([sys.executable, checker], cwd=project, capture_output=True, text=True, env=env,
                              timeout=120, check=False)
        passed = done.returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        passed = False
    finally:
        os.remove(checker)
    return {"files": files, "check_passed": passed, "commits": git.commits(project), "dirty": git.dirty(project)}
