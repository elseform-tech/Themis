"""Calls to the real `themis` CLI and app server, and keeping a run's evidence.

Copied from `evals/hard/app.py` (`_cli`, `keep_evidence`) and `evals/budget/execute.py`
(`build_app`, `_kill`). Every call names the run's own data folder; the environment comes from
`evals.kit.env.isolated_env`, so no Keychain and no real app data are touched.
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

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
EXE = ".exe" if os.name == "nt" else ""


def themis_binary(root=ROOT):
    """The debug `themis` CLI from this checkout (built by the caller)."""
    target = os.environ.get("CARGO_TARGET_DIR") or os.path.join(root, "target")
    return os.path.join(target, "debug", "themis" + EXE)


def build_app(root=ROOT):
    built = subprocess.run(["cargo", "build", "-q", "-p", "themis-desktop", "--bin", "themis"], cwd=root,
                           check=False)
    if built.returncode != 0:
        sys.exit(built.returncode)
    return themis_binary(root)


def cli(binary, data, env, *args, stdin=None, timeout=120):
    """One CLI command against the data folder `data` (CompletedProcess; never raises on exit)."""
    return subprocess.run([binary, "--data-dir", data, *args], input=stdin, capture_output=True,
                          text=True, env=env, timeout=timeout, check=False)


def call(binary, data, env, method, args=None, timeout=120):
    """(value, error): `themis call METHOD JSON_ARGS`, the JSON value it printed or the error text.
    A command that does not answer within `timeout` is an error, not an exception."""
    try:
        done = cli(binary, data, env, "call", method, json.dumps(args if args is not None else {}),
                   timeout=timeout)
    except subprocess.TimeoutExpired:
        return None, f"{method}: no answer in {timeout}s"
    if done.returncode != 0:
        return None, f"{method}: {(done.stderr or done.stdout).strip()[-300:]}"
    text = done.stdout.strip()
    if not text:
        return None, None
    try:
        return json.loads(text), None
    except ValueError:
        return None, f"{method}: not JSON: {text[-200:]}"


def kill(process):
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except (OSError, AttributeError):
        process.kill()


def keep_evidence(workdir, keep, name):
    """Copies a finished run's folders under `keep`. Links stay links (the built-in browser's
    profile holds a dangling `RunningChromeVersion`), and a copy that still fails is reported
    without replacing the run's result."""
    try:
        os.makedirs(keep, exist_ok=True)
        shutil.copytree(workdir, tempfile.mkdtemp(prefix=f"{name}-", dir=keep), symlinks=True,
                        dirs_exist_ok=True, ignore=shutil.ignore_patterns("vendor", "__pycache__", ".git"))
    except (OSError, shutil.Error) as error:
        print(f"could not keep {name}: {str(error)[:200]}", file=sys.stderr, flush=True)


TERMINAL = ("finished", "failed", "incomplete")


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
                # The app repeats the goal every minute while it runs: that tick is not activity.
                if payload["event"].get("kind") != "goal_changed":
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
        kill(self.process)
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            pass
        self.reader.join(timeout=5)
