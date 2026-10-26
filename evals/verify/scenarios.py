"""The scenarios: each folder under `scenarios/` holds `scenario.json` and the `hidden/` checks the
agent never sees. The project itself comes from `generate.py` (the scenario's planted fault on a
seeded Ledgerly).

scenario.json: `kind` (bug, feature, refactor, web or lookup), `message` (the user's request),
`source` (files the change belongs in), `tests` (the project's test folder), `hidden_command`
(run in a copy of the finished project with the hidden checks under `_hidden/`), optionally
`plants` (the plants that make its starting point; its own name by default) and `script` (its
own extra messages, sent under every pressure profile; see pressure.py), and for lookups
`answer_patterns` and `evidence_hosts`.
"""

import json
import os
import re
import shutil
import subprocess
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
SCENARIOS = os.path.join(HERE, "scenarios")
HIDDEN = "_hidden"
SUMMARY = re.compile(r"(\d+) (passed|failed|error|errors|skipped)")


def names():
    return sorted(name for name in os.listdir(SCENARIOS)
                  if os.path.isfile(os.path.join(SCENARIOS, name, "scenario.json")))


def load(name):
    with open(os.path.join(SCENARIOS, name, "scenario.json"), encoding="utf-8") as handle:
        scenario = json.load(handle)
    scenario["name"] = name
    scenario["hidden"] = os.path.join(SCENARIOS, name, "hidden")
    # The plants that make the scenario's starting point; by default its own name.
    scenario.setdefault("plants", [name])
    return scenario


def run_hidden(scenario, project, env=None, timeout=300):
    """{"passed", "total", "line"}: the hidden checks against a copy of `project`."""
    with tempfile.TemporaryDirectory(prefix="verify-grade-") as scratch:
        copy = os.path.join(scratch, "project")
        shutil.copytree(project, copy, ignore=shutil.ignore_patterns(".git", "__pycache__", HIDDEN, ".pytest_cache"))
        shutil.copytree(scenario["hidden"], os.path.join(copy, HIDDEN))
        return pytest_counts(scenario["hidden_command"], copy, env, timeout)


def run_project_tests(project, tests="tests", env=None, timeout=300):
    """{"passed", "total", "line"}: the project's own tests as they stand."""
    if not os.path.isdir(os.path.join(project, tests)):
        return {"passed": 0, "total": 0, "line": "no tests"}
    with tempfile.TemporaryDirectory(prefix="verify-grade-") as scratch:
        copy = os.path.join(scratch, "project")
        shutil.copytree(project, copy, ignore=shutil.ignore_patterns(".git", "__pycache__", HIDDEN, ".pytest_cache"))
        return pytest_counts(["python3", "-m", "pytest", "-q", "-p", "no:cacheprovider", tests], copy, env, timeout)


def pytest_counts(command, cwd, env, timeout):
    environment = dict(env or os.environ)
    environment["PYTHONPATH"] = cwd
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    try:
        done = subprocess.run(command, cwd=cwd, env=environment, capture_output=True, text=True,
                              timeout=timeout, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"passed": 0, "total": 1, "line": type(error).__name__}
    lines = [line for line in (done.stdout + done.stderr).splitlines() if line.strip()]
    last = lines[-1] if lines else ""
    counts = {kind: int(number) for number, kind in SUMMARY.findall(last)}
    passed = counts.get("passed", 0)
    failed = counts.get("failed", 0) + counts.get("error", 0) + counts.get("errors", 0)
    total = passed + failed
    if total == 0:
        # A collection error or crash: count the whole check as one failure.
        total = 1
    return {"passed": passed, "total": total, "line": last[:300]}
