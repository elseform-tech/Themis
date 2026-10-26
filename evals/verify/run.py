"""Runs one verification task through the real app, as a person's chat runs, and records its trace.

A scratch HOME and data folder (no Keychain, no real app data) and a git project copied from the
task; then through the `themis` CLI: a chat in Custom mode with `APPROVAL` (the OS sandbox stays
on, so commands write only inside the project; nothing waits for an approval), the task's message with `send_message`, events from `themis watch` until the run ends. The finished
project is graded by the task's hidden check, which the agent never sees.

Spend is metered live from `context_usage` events and the app's usage ledger; a run past its cost
cap or time limit is stopped and recorded with that end.
"""

import json
import os
import shutil
import tempfile
import threading
import time

from evals.kit import app, git
from evals.kit import spend as spend_kit
from evals.kit.app import Watch
from evals.kit.env import isolated_env

from . import tasks

# Custom-mode rules: enough to read, change, test and look things up, nothing outside the
# sandbox. The default is deny, so no approval ever waits for a person who is not there.
APPROVAL = {
    "default": "deny",
    "rules": [{"tool": "shell_unsandboxed", "action": "deny"},
              {"tool": "*", "risk": "read", "action": "allow"}]
             + [{"tool": "shell", "command": [program], "action": "allow"} for program in ("python3", "node")]
             + [{"tool": tool, "action": "allow"}
                for tool in ("write_file", "apply_patch", "git", "web_search", "fetch_url", "browser_*")],
}


class VerifyRun:
    """One run: `options` holds model, timeout, max_run_cost, keep, searxng (a packed SearXNG
    folder or None)."""

    def __init__(self, binary, options, task, rep, price):
        self.binary, self.options, self.task, self.rep = binary, options, task, rep
        self.meter = spend_kit.LedgerMeter(price) if price else None
        self.stop_reason = None
        self._stop_lock = threading.Lock()

    def call(self, method, args=None, timeout=60):
        return app.call(self.binary, self.data, self.env, method, args, timeout=timeout)

    def halt(self, reason):
        with self._stop_lock:
            if self.stop_reason is not None:
                return
            self.stop_reason = reason
        self.call("stop_thread", {"threadId": self.thread}, timeout=30)

    def on_event(self, item):
        event = item["event"]
        if event.get("kind") == "context_usage" and self.meter is not None:
            self.meter.update(item["run_id"], spend_kit.metrics_counts(event.get("metrics") or {}))
            if not self.meter.price.free and self.meter.cost() > self.options["max_run_cost"]:
                threading.Thread(target=self.halt, args=("cap_cost",), daemon=True).start()

    def run(self):
        started = time.monotonic()
        self.workdir = tempfile.mkdtemp(prefix="verify-")
        self.data = os.path.join(self.workdir, "data")
        self.project = os.path.join(self.workdir, "project")
        os.makedirs(self.data)
        shutil.copytree(self.task["project"], self.project)
        self.env = isolated_env(self.workdir)
        if self.options.get("searxng"):
            self.env["THEMIS_SEARXNG_DIR"] = self.options["searxng"]
        git.init(self.project)
        with open(os.path.join(self.data, "runtime.jsonc"), "w", encoding="utf-8") as handle:
            json.dump({"schema_version": 1, "approval": APPROVAL}, handle)
        trace = {"task": self.task["name"], "kind": self.task["kind"], "rep": self.rep,
                 "model": self.options["model"], "events": [], "answer": "", "end": None,
                 "tests_before": tasks.test_count(self.project, self.task.get("tests"))}
        self.thread, self.watch = None, None
        try:
            trace["end"] = self._drive(trace, started)
        except Exception as error:  # noqa: BLE001 - a runner error must still leave a trace
            trace["end"] = {"reason": "crash", "detail": f"runner: {type(error).__name__}: {str(error)[:200]}"}
        finally:
            if self.watch is not None:
                trace["events"] = self.watch.snapshot()
            if self.meter is not None:
                summary, error = self.call("get_usage_summary", None, timeout=30)
                if not error:
                    self.meter.update_ledger(spend_kit.ledger_counts(summary))
                trace["usage"], trace["cost"] = self.meter.usage(), self.meter.cost()
            app.cli(self.binary, self.data, self.env, "server", "stop")
            if self.watch is not None:
                self.watch.stop()
            trace["answer"] = final_answer(trace["events"])
            passed, line = tasks.run_hidden(self.task, self.project, env=self.env)
            trace["hidden"] = {"passed": passed, "line": line}
            trace["tests_after"] = tasks.test_count(self.project, self.task.get("tests"))
            if self.task["kind"] == "bug":
                trace["tests_catch_original"] = tasks.tests_catch_original(self.task, self.project, env=self.env)
            trace["wall_s"] = round(time.monotonic() - started, 1)
            if self.options.get("keep"):
                app.keep_evidence(self.workdir, self.options["keep"], f"verify-{self.task['name']}-{self.rep}")
            shutil.rmtree(self.workdir, ignore_errors=True)
        return trace

    def _drive(self, trace, started):
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
        self.watch = Watch(self.binary, self.data, self.env, self.thread, self.on_event)
        time.sleep(2)  # let the subscription start before the run
        handle, error = self.call("send_message", {"threadId": self.thread, "text": self.task["message"]})
        if error or not isinstance(handle, dict):
            return {"reason": "infrastructure", "detail": f"send: {error}"}
        deadline = started + self.options["timeout"]
        while time.monotonic() < deadline:
            end = self.watch.run_end(handle["run_id"])
            if end is not None:
                if end.get("kind") == "failed":
                    return {"reason": self.stop_reason or "failed", "detail": str(end.get("error") or "")[:300]}
                return {"reason": self.stop_reason or "finished", "detail": ""}
            if not self.watch.alive():
                return {"reason": "crash", "detail": "event stream closed"}
            time.sleep(1)
        self.halt("cap_time")
        return {"reason": "cap_time", "detail": ""}


def final_answer(events):
    """The run's reply: its `finished` result, else its last assistant text."""
    for item in reversed(events):
        event = item["event"]
        if event.get("kind") == "finished" and event.get("result"):
            return event["result"]
    texts = [item["event"].get("text") or "" for item in events if item["event"].get("kind") == "assistant_text"]
    return texts[-1] if texts else ""
