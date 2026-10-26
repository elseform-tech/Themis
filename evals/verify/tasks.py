"""The verification tasks: each folder under `tasks/` holds `task.json`, the `project/` the agent
works in and the `hidden/` checks it never sees.

task.json: `kind` (bug, feature, web or lookup), `message` (the user's request), `source` (the
files the fix belongs in), `hidden_command` (run in a copy of the finished project with the
hidden files under `_hidden/`; exit 0 means correct), and optionally `tests` (the project's test
folder), `answer_patterns` and `evidence_hosts` (lookup tasks).
"""

import json
import os
import shutil
import subprocess
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
TASKS = os.path.join(HERE, "tasks")
HIDDEN = "_hidden"


def names():
    return sorted(name for name in os.listdir(TASKS) if os.path.isfile(os.path.join(TASKS, name, "task.json")))


def load(name):
    with open(os.path.join(TASKS, name, "task.json"), encoding="utf-8") as handle:
        task = json.load(handle)
    task["name"] = name
    task["project"] = os.path.join(TASKS, name, "project")
    task["hidden"] = os.path.join(TASKS, name, "hidden")
    return task


def run_hidden(task, project, env=None, timeout=120):
    """(passed, last output line): the hidden check against a copy of `project`."""
    with tempfile.TemporaryDirectory(prefix="verify-grade-") as scratch:
        copy = os.path.join(scratch, "project")
        shutil.copytree(project, copy, ignore=shutil.ignore_patterns(".git", "__pycache__", HIDDEN))
        shutil.copytree(task["hidden"], os.path.join(copy, HIDDEN))
        return _run(task["hidden_command"], copy, env, timeout)


def tests_catch_original(task, project, env=None, timeout=120):
    """Whether the project's own tests, as the agent left them, fail on the original source:
    True when they catch the bug, False when they pass on it, None without a test folder."""
    tests = task.get("tests")
    if not tests or not os.path.isdir(os.path.join(project, tests)):
        return None
    with tempfile.TemporaryDirectory(prefix="verify-grade-") as scratch:
        copy = os.path.join(scratch, "project")
        shutil.copytree(project, copy, ignore=shutil.ignore_patterns(".git", "__pycache__"))
        for path in task["source"]:
            shutil.copyfile(os.path.join(task["project"], path), os.path.join(copy, path))
        passed, _line = _run(["python3", "-m", "pytest", "-q", "-p", "no:cacheprovider", tests], copy, env, timeout)
        return not passed


def test_count(project, tests):
    """`def test_` functions under the project's test folder."""
    total = 0
    folder = os.path.join(project, tests or "")
    if not tests or not os.path.isdir(folder):
        return 0
    for base, _dirs, files in os.walk(folder):
        for name in files:
            if name.endswith(".py"):
                with open(os.path.join(base, name), encoding="utf-8", errors="replace") as handle:
                    total += sum(line.lstrip().startswith("def test_") for line in handle)
    return total


def _run(command, cwd, env, timeout):
    environment = dict(env or os.environ)
    environment["PYTHONPATH"] = cwd
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    try:
        done = subprocess.run(command, cwd=cwd, env=environment, capture_output=True, text=True,
                              timeout=timeout, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        return False, f"{type(error).__name__}"
    lines = [line for line in (done.stdout + done.stderr).splitlines() if line.strip()]
    return done.returncode == 0, (lines[-1] if lines else "")[:300]
