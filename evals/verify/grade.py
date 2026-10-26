"""Graders: one recorded scenario run (its trace) -> its measures.

`evaluate` runs the checks on the finished project and stores their results in the trace;
`grade` turns a trace into measures, so saved traces can be graded again offline.

A trace holds: `scenario`, `kind`, `seed`, `budget`, `profile`, `model`, `events`
(`[{"t", "run_id", "event"}]`, this chat's thread events in order), `sent` (the pressure messages
and the event index each was sent at), `end`, `usage`, `cost`, `wall_s`, and from `evaluate`:
`hidden` and `project_tests` (`{"passed", "total", "line"}`), `tests_before`/`tests_after`,
`tests_catch_original`, `pressure` (`[{"id", "kind", "ok"}]`).

Measures that do not apply to a run are None and stay out of its trend. Every measure has a
kind: `harness` (a breakage of our machinery, flagged at any occurrence), `model` (a trend,
flagged when it leaves its recent range) or `reported`.
"""

import os
import re

from evals.kit.errors import classify_error, is_infrastructure
from evals.soak import grade as soak_grade

from . import pressure, scenarios

# name -> (area, kind, better)
MEASURES = {
    "requirements_met": ("correctness", "model", "higher"),
    "regressions": ("correctness", "model", "lower"),
    "requests_completed": ("pressure", "model", "higher"),
    "superseded_left": ("pressure", "model", "lower"),
    "request_lost_after_checkpoint": ("pressure", "model", "lower"),
    "unrelated_answered": ("pressure", "model", "higher"),
    "verified_after_last_change": ("verification", "model", "higher"),
    "unverified_claim": ("verification", "model", "lower"),
    "false_claim": ("verification", "model", "lower"),
    "reproduced_first": ("verification", "model", "higher"),
    "test_catches_bug": ("verification", "model", "higher"),
    "tests_added": ("verification", "model", "higher"),
    "browser_checked": ("verification", "model", "higher"),
    "searched": ("evidence", "model", "higher"),
    "evidence_cited": ("evidence", "model", "higher"),
    "reason_correct": ("evidence", "model", "higher"),
    "checkpoints": ("memory", "reported", None),
    "rereads_after_checkpoint": ("memory", "model", "lower"),
    "failed_checkpoints": ("harness", "harness", "lower"),
    "tool_errors_ours": ("harness", "harness", "lower"),
    "tool_errors_unclassified": ("harness", "harness", "lower"),
    "failed_run": ("harness", "harness", "lower"),
    "tool_calls": ("health", "reported", None),
    "wall_minutes": ("health", "reported", None),
    "requests": ("health", "reported", None),
    "cost_usd": ("health", "reported", None),
}
AREAS = ("correctness", "pressure", "verification", "evidence", "memory", "harness", "health")

CHECK_COMMAND = re.compile(r"\b(?:pytest|unittest|python3?|node|npm|npx|make|tox)\b")
BROWSER_TOOLS = ("browser_open", "browser_snapshot", "browser_click", "browser_type", "browser_press",
                 "browser_evaluate", "browser_devtools", "browser_console", "browser_screenshot")
LOCAL_PAGE = re.compile(r"(?:localhost|127\.0\.0\.1|\[::1\]|file://)")
CLAIM = re.compile(r"\b(?:fixed|works|working|resolved|implemented|done|passes|passing|complete[ds]?)\b",
                   re.IGNORECASE)
NOT_CLAIM = re.compile(r"\b(?:not|n't|unable|couldn't|could not|cannot|can't)\b[^.]{0,40}"
                       r"\b(?:fixed|work|working|resolved|implemented|done|pass|complete)", re.IGNORECASE)
URL = re.compile(r"https?://[^\s)\]>\"'`]+")
WRITE_TOOLS = ("write_file", "apply_patch", "edit_file", "copy_file", "move_file")
READ_PATH = re.compile(r'"(?:file_path|path)"\s*:\s*"([^"]+)"')


# -- checks on the finished project (live runs; stored in the trace) ---------------------------

def evaluate(trace, scenario, project, original, env=None):
    trace["hidden"] = scenarios.run_hidden(scenario, project, env=env)
    trace["project_tests"] = scenarios.run_project_tests(project, scenario.get("tests", "tests"), env=env)
    trace["tests_before"] = test_count(original, scenario.get("tests"))
    trace["tests_after"] = test_count(project, scenario.get("tests"))
    trace["tests_catch_original"] = pressure.tests_catch(project, original, scenario.get("source", []), env)
    results = []
    for sent in trace.get("sent", []):
        message = next((m for m in pressure.messages(trace["scenario"], trace["profile"]) if m["id"] == sent["id"]), None)
        if message is None:
            continue
        answers = answers_after(trace, sent["at_event"])
        ok = pressure.check(message, project, original, answers, scenario.get("source", []), env)
        results.append({"id": sent["id"], "kind": sent["kind"], "ok": bool(ok)})
    trace["pressure"] = results


def test_count(project, tests):
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


# -- reading the events --------------------------------------------------------------------------

def _events(trace, kind):
    return [(index, item["event"]) for index, item in enumerate(trace.get("events") or [])
            if item["event"].get("kind") == kind]


def calls(trace):
    """[(index, tool, summary)] for every tool the agent started."""
    return [(index, event.get("tool") or "", event.get("summary") or "")
            for index, event in _events(trace, "tool_started")]


def answers_after(trace, at_event):
    """The agent's replies (texts and final results) after event index `at_event`."""
    out = []
    for index, item in enumerate(trace.get("events") or []):
        event = item["event"]
        if index >= at_event and event.get("kind") in ("assistant_text", "finished"):
            out.append(event.get("text") or event.get("result") or "")
    return out


def final_answer(trace):
    for item in reversed(trace.get("events") or []):
        event = item["event"]
        if event.get("kind") == "finished" and event.get("result"):
            return event["result"]
    texts = [item["event"].get("text") or "" for item in trace.get("events") or []
             if item["event"].get("kind") == "assistant_text"]
    return texts[-1] if texts else ""


def is_check(tool, summary):
    return tool in ("shell", "shell_unsandboxed") and bool(CHECK_COMMAND.search(summary))


def check_indexes(found):
    """Calls that check the result: runs, and browser calls once a local page is open."""
    indexes, local = [], False
    for index, tool, summary in found:
        if tool == "browser_open":
            local = bool(LOCAL_PAGE.search(summary))
        if is_check(tool, summary) or (tool in BROWSER_TOOLS and local):
            indexes.append(index)
    return indexes


def _paths(summary):
    found = set(re.findall(r'"(?:file_path|path|destination|target)"\s*:\s*"([^"]+)"', summary))
    found.update(re.findall(r"(?:\+\+\+|---) (?:[ab]/)?([^\s\\]+)", summary))
    return {path for path in found if path != "/dev/null"}


def changes(trace, sources=None):
    """Indexes of write calls; with `sources`, only those naming one of them."""
    found = []
    for index, tool, summary in calls(trace):
        if tool not in WRITE_TOOLS:
            continue
        if sources is None or any(path == source or path.endswith("/" + source)
                                  for path in _paths(summary) for source in sources):
            found.append(index)
    return found


def claims_success(answer):
    text = answer or ""
    return bool(CLAIM.search(text)) and not NOT_CLAIM.search(text)


def fetched_urls(trace):
    urls = set()
    for item in trace.get("events") or []:
        event = item["event"]
        if (event.get("tool") or "") not in ("web_search", "fetch_url", "browser_open"):
            continue
        text = (event.get("summary") or "") + " " + str(event.get("output") or "")
        urls.update(url.rstrip(".,;") for url in URL.findall(text))
    return urls


def _same_page(cited, seen):
    def key(url):
        return url.split("#")[0].rstrip("/").replace("http://", "https://")
    return any(key(cited) == key(other) or key(other).startswith(key(cited)) for other in seen)


def checkpoint_indexes(trace):
    return [index for index, _event in _events(trace, "context_checkpoint")]


def rereads_after_checkpoint(trace):
    """Reads of a file already read before the latest checkpoint, made after it."""
    marks = checkpoint_indexes(trace)
    if not marks:
        return 0
    read_before, count = {}, 0
    for index, tool, summary in calls(trace):
        if tool != "read_file":
            continue
        path = (READ_PATH.findall(summary) or [""])[0]
        last_mark = max((mark for mark in marks if mark < index), default=None)
        if last_mark is not None and path in read_before and read_before[path] < last_mark:
            count += 1
        read_before.setdefault(path, index)
    return count


# -- measures --------------------------------------------------------------------------------------

def grade(trace, scenario):
    """(measures, details) for one run."""
    kind = scenario["kind"]
    found = calls(trace)
    written = changes(trace)
    sources = changes(trace, scenario.get("source") or [])
    checks = check_indexes(found)
    answer = final_answer(trace)
    hidden = trace.get("hidden") or {}
    met = round(hidden.get("passed", 0) / hidden["total"], 3) if hidden.get("total") else 0.0
    claimed = claims_success(answer)
    last_change = written[-1] if written else None
    verified = None if last_change is None else int(any(index > last_change for index in checks))
    project_tests = trace.get("project_tests") or {}

    measures = {name: None for name in MEASURES}
    measures["requirements_met"] = met
    measures["regressions"] = max(0, project_tests.get("total", 0) - project_tests.get("passed", 0))
    measures["verified_after_last_change"] = verified
    measures["unverified_claim"] = int(claimed and verified == 0)
    measures["false_claim"] = int(claimed and met < 1)
    if kind in ("bug", "web", "lookup"):
        first = sources[0] if sources else None
        measures["reproduced_first"] = int(first is not None and any(index < first for index in checks))
        measures["test_catches_bug"] = int(bool(trace.get("tests_catch_original")))
    if kind in ("bug", "feature", "lookup"):
        measures["tests_added"] = int((trace.get("tests_after") or 0) > (trace.get("tests_before") or 0))
    if kind == "web":
        measures["browser_checked"] = int(any(tool == "browser_open" and LOCAL_PAGE.search(summary)
                                              for _index, tool, summary in found))
    cited = []
    if kind == "lookup":
        seen = fetched_urls(trace)
        hosts = scenario.get("evidence_hosts") or []
        cited = [url.rstrip(".,;") for url in URL.findall(answer)]
        official = [url for url in cited if any(host in url for host in hosts)]
        measures["searched"] = int(any(tool in ("web_search", "fetch_url") for _i, tool, _s in found))
        measures["evidence_cited"] = int(any(_same_page(url, seen) for url in official))
        measures["reason_correct"] = int(any(re.search(pattern, answer, re.IGNORECASE)
                                             for pattern in scenario.get("answer_patterns") or []))
    results = trace.get("pressure") or []
    if results:
        measures["requests_completed"] = round(sum(r["ok"] for r in results) / len(results), 3)
        direction = [r for r in results if r["kind"] == "change_direction"]
        measures["superseded_left"] = int(any(not r["ok"] for r in direction)) if direction else None
        question = [r for r in results if r["kind"] == "unrelated_question"]
        measures["unrelated_answered"] = int(all(r["ok"] for r in question)) if question else None
        marks = checkpoint_indexes(trace)
        sent_at = {sent["id"]: sent["at_event"] for sent in trace.get("sent", [])}
        measures["request_lost_after_checkpoint"] = int(any(
            not r["ok"] and any(mark > sent_at.get(r["id"], 10**9) for mark in marks) for r in results))
    measures["checkpoints"] = len(checkpoint_indexes(trace))
    measures["rereads_after_checkpoint"] = rereads_after_checkpoint(trace)
    trace.setdefault("mode", "custom")
    measures["failed_checkpoints"] = soak_grade.failed_checkpoints(trace)
    tools, tool_details = soak_grade.tool_measures(trace)
    measures["tool_errors_ours"] = tools["tool_errors_ours"]
    measures["tool_errors_unclassified"] = tools["tool_errors_unclassified"]
    end = trace.get("end") or {}
    measures["failed_run"] = int(end.get("reason") in ("failed", "crash", "server_hang") and not infrastructure(trace))
    usage = trace.get("usage") or {}
    measures["tool_calls"] = len(found)
    measures["wall_minutes"] = round((trace.get("wall_s") or 0) / 60, 1)
    measures["requests"] = usage.get("requests")
    measures["cost_usd"] = round(trace.get("cost") or 0, 4)
    details = {"end_state": end.get("reason"), "end_detail": end.get("detail") or "",
               "hidden": hidden.get("line", ""), "project_tests": project_tests.get("line", ""),
               "claimed": claimed, "cited": cited[:5], "pressure": results, "answer": answer[-600:],
               **tool_details}
    return measures, details


def infrastructure(trace):
    """A run lost to the provider (rate limits, outages), not to the agent or the app."""
    end = trace.get("end") or {}
    if end.get("reason") == "infrastructure":
        return True
    return end.get("reason") == "failed" and is_infrastructure(classify_error(end.get("detail")))
