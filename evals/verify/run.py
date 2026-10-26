"""Runs one verification scenario through the real app, as a person's chat runs, and records its
trace.

A scratch HOME and data folder (no Keychain, no real app data); a generated Ledgerly project for
(scenario, seed) with the pressure profile's changes not yet made; the run's context budget and
Custom-mode rules (`APPROVAL`: the OS sandbox stays on, so commands write only inside the
project; nothing waits for an approval) in the data folder's `runtime.jsonc`. Then through the
`themis` CLI: a chat, the scenario's message with `send_message`, events from `themis watch`,
and the profile's messages sent as the agent's tool calls pass their thresholds: queued with
`enqueue_message`, and steered into the running turn with `steer_queued_message` when the message
says so. The run ends when every message is sent, no turn runs and the queue is empty.

Spend is metered live from `context_usage` events and the app's usage ledger; a run past its cost
cap or time limit is stopped and recorded with that end.
"""

import json
import os
import shutil
import tempfile
import threading
import time

from evals.kit import app
from evals.kit import spend as spend_kit
from evals.kit.app import TERMINAL, Watch
from evals.kit.env import isolated_env

from . import generate, pressure

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

    def __init__(self, binary, options, scenario, seed, budget, profile, price):
        self.binary, self.options, self.scenario = binary, options, scenario
        self.seed, self.budget, self.profile = seed, budget, profile
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
        self.original = os.path.join(self.workdir, "original")
        os.makedirs(self.data)
        name = self.scenario["name"]
        generate.project(self.project, name, self.seed, git=True, extra=pressure.plants(name, self.profile))
        shutil.copytree(self.project, self.original, ignore=shutil.ignore_patterns(".git"))
        self.env = isolated_env(self.workdir)
        if self.options.get("searxng"):
            self.env["THEMIS_SEARXNG_DIR"] = self.options["searxng"]
        with open(os.path.join(self.data, "runtime.jsonc"), "w", encoding="utf-8") as handle:
            json.dump({"schema_version": 1, "context_token_budget": self.budget, "approval": APPROVAL}, handle)
        trace = {"scenario": name, "kind": self.scenario["kind"], "seed": self.seed, "budget": self.budget,
                 "profile": self.profile, "model": self.options["model"], "events": [], "sent": [], "end": None}
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
            trace["wall_s"] = round(time.monotonic() - started, 1)
            from .grade import evaluate  # the checks run on the finished project

            evaluate(trace, self.scenario, self.project, self.original, self.env)
            if self.options.get("keep"):
                app.keep_evidence(self.workdir, self.options["keep"],
                                  f"verify-{name}-{self.budget}-{self.profile}-{self.seed}")
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
        handle, error = self.call("send_message", {"threadId": self.thread, "text": self.scenario["message"]})
        if error or not isinstance(handle, dict):
            return {"reason": "infrastructure", "detail": f"send: {error}"}
        pending = list(pressure.messages(self.scenario["name"], self.profile))
        deadline = started + self.options["timeout"]
        idle_since = None
        while time.monotonic() < deadline:
            if self.stop_reason:
                return {"reason": self.stop_reason, "detail": ""}
            if not self.watch.alive():
                return {"reason": "crash", "detail": "event stream closed"}
            events = self.watch.snapshot()
            calls = sum(item["event"].get("kind") == "tool_started" for item in events)
            running = active_run(events)
            began = any(item["event"].get("kind") == "started" for item in events)
            if not began:
                time.sleep(1)
                continue
            while pending and (calls >= pending[0]["after_calls"] or not running):
                message = pending.pop(0)
                self._send(trace, message, running, len(events))
                running = True
            if not running and not pending:
                queue, _error = self.call("list_queue", {"threadId": self.thread}, timeout=30)
                if not queue:
                    # A queued message can take a moment to start its run: wait out a short grace.
                    idle_since = idle_since or time.monotonic()
                    if time.monotonic() - idle_since > 8:
                        failed = last_failure(events)
                        return {"reason": "failed", "detail": failed} if failed else {"reason": "finished", "detail": ""}
                else:
                    idle_since = None
            else:
                idle_since = None
            time.sleep(1)
        self.halt("cap_time")
        return {"reason": "cap_time", "detail": ""}

    def _send(self, trace, message, running, at_event):
        queued, error = self.call("enqueue_message", {"threadId": self.thread, "text": message["text"]})
        steered = False
        if not error and running and message["mode"] == "steer" and isinstance(queued, dict):
            _value, steer_error = self.call("steer_queued_message", {"threadId": self.thread, "messageId": queued["id"]})
            steered = not steer_error
        trace["sent"].append({"id": message["id"], "kind": message["kind"], "at_event": at_event,
                              "steered": steered, "error": error})


def active_run(events):
    """Whether a turn has started and not yet ended."""
    open_runs = set()
    for item in events:
        kind = item["event"].get("kind")
        if kind == "started":
            open_runs.add(item["run_id"])
        elif kind in TERMINAL:
            open_runs.discard(item["run_id"])
    return bool(open_runs)


def last_failure(events):
    for item in reversed(events):
        if item["event"].get("kind") == "failed":
            return str(item["event"].get("error") or "")[:300]
        if item["event"].get("kind") in TERMINAL:
            return ""
    return ""
