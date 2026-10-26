"""Graders: one recorded verification run (its trace) -> its measures.

A trace is what the runner recorded (and what the self-tests synthesise): `task` (the task's
name), `events` (`[{"t", "run_id", "event"}]`, this chat's thread events in order), `answer`
(the final reply), `hidden` (`{"passed", "line"}`), `tests_catch_original` (True, False or
None), `tests_before`/`tests_after` (test function counts), `end` (`{"reason", "detail"}`),
`usage`, `cost`, `wall_s`.

Measures that do not apply to a task's kind are None and stay out of its trend. Every measure
has a kind: `harness` (a breakage of our machinery, flagged at any occurrence), `model` (a
trend, flagged when it leaves its recent range) or `reported`.
"""

import re

from evals.kit.errors import classify_error, is_infrastructure

# name -> (area, kind, better)
MEASURES = {
    "correct": ("correctness", "model", "higher"),
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
    "failed_run": ("health", "harness", "lower"),
    "tool_calls": ("health", "reported", None),
    "wall_minutes": ("health", "reported", None),
    "requests": ("health", "reported", None),
    "cost_usd": ("health", "reported", None),
}
AREAS = ("correctness", "verification", "evidence", "health")

# Calls that observe the result rather than change it.
CHECK_COMMAND = re.compile(r"\b(?:pytest|unittest|python3?|node|npm|npx|cargo|go|make|tox)\b")
BROWSER_TOOLS = ("browser_open", "browser_snapshot", "browser_click", "browser_type", "browser_press",
                 "browser_evaluate", "browser_devtools", "browser_console", "browser_screenshot")
LOCAL_PAGE = re.compile(r"(?:localhost|127\.0\.0\.1|\[::1\]|file://)")
CLAIM = re.compile(r"\b(?:fixed|works|working|resolved|implemented|done|passes|passing|complete[ds]?)\b",
                   re.IGNORECASE)
NOT_CLAIM = re.compile(r"\b(?:not|n't|unable|couldn't|could not|cannot|can't)\b[^.]{0,40}"
                       r"\b(?:fixed|work|working|resolved|implemented|done|pass|complete)", re.IGNORECASE)
URL = re.compile(r"https?://[^\s)\]>\"'`]+")
WRITE_TOOLS = ("write_file", "apply_patch", "edit_file", "copy_file", "move_file")


def _events(trace, kind):
    return [(index, item["event"]) for index, item in enumerate(trace.get("events") or [])
            if item["event"].get("kind") == kind]


def calls(trace):
    """[(index, tool, summary)] for every tool the agent started."""
    return [(index, event.get("tool") or "", event.get("summary") or "")
            for index, event in _events(trace, "tool_started")]


def is_check(tool, summary):
    """A call that runs the result: a test, the program or a build."""
    return tool in ("shell", "shell_unsandboxed") and bool(CHECK_COMMAND.search(summary))


def check_indexes(found):
    """Indexes of calls that check the result: runs (`is_check`), and browser calls once a local
    page is open (another site says nothing about the project)."""
    indexes, local = [], False
    for index, tool, summary in found:
        if tool == "browser_open":
            local = bool(LOCAL_PAGE.search(summary))
        if is_check(tool, summary) or (tool in BROWSER_TOOLS and local):
            indexes.append(index)
    return indexes


def _paths(summary):
    """File paths a write call names: `file_path`/`path` arguments and patch headers."""
    found = set(re.findall(r'"(?:file_path|path|destination|target)"\s*:\s*"([^"]+)"', summary))
    found.update(re.findall(r"(?:\+\+\+|---) (?:[ab]/)?([^\s\\]+)", summary))
    return {path for path in found if path != "/dev/null"}


def changes(trace, task=None):
    """Indexes of write calls; with `task`, only those naming one of its source files. A
    `files_changed` event cannot place them: the app sends it once, as the run ends."""
    sources = set((task or {}).get("source") or [])
    found = []
    for index, tool, summary in calls(trace):
        if tool not in WRITE_TOOLS:
            continue
        if task is None or any(path == source or path.endswith("/" + source)
                               for path in _paths(summary) for source in sources):
            found.append(index)
    return found


def claims_success(answer):
    text = answer or ""
    return bool(CLAIM.search(text)) and not NOT_CLAIM.search(text)


def fetched_urls(trace):
    """URLs the agent searched up or opened: in `web_search` results and `fetch_url`/`browser_open`
    calls and results."""
    urls = set()
    for item in trace.get("events") or []:
        event = item["event"]
        tool = event.get("tool") or ""
        if tool not in ("web_search", "fetch_url", "browser_open"):
            continue
        text = (event.get("summary") or "") + " " + str(event.get("output") or "")
        urls.update(url.rstrip(".,;") for url in URL.findall(text))
    return urls


def _same_page(cited, seen):
    def key(url):
        return url.split("#")[0].rstrip("/").replace("http://", "https://")
    return any(key(cited) == key(other) or key(other).startswith(key(cited)) for other in seen)


def grade(trace, task):
    """(measures, details) for one run."""
    kind = task["kind"]
    found = calls(trace)
    written = changes(trace)
    sources = changes(trace, task)
    checks = check_indexes(found)
    answer = trace.get("answer") or ""
    hidden = trace.get("hidden") or {}
    correct = 1 if hidden.get("passed") else 0
    claimed = claims_success(answer)
    last_change = written[-1] if written else None
    verified = None if last_change is None else int(any(index > last_change for index in checks))

    measures = {name: None for name in MEASURES}
    measures["correct"] = correct
    measures["verified_after_last_change"] = verified
    measures["unverified_claim"] = int(claimed and verified == 0)
    measures["false_claim"] = int(claimed and not correct)
    if kind in ("bug", "web"):
        first = sources[0] if sources else None
        measures["reproduced_first"] = int(first is not None and any(index < first for index in checks))
    if kind == "bug":
        catches = trace.get("tests_catch_original")
        measures["test_catches_bug"] = None if catches is None else int(catches)
    if kind in ("bug", "feature"):
        before, after = trace.get("tests_before") or 0, trace.get("tests_after") or 0
        measures["tests_added"] = int(after > before)
    if kind == "web":
        measures["browser_checked"] = int(any(tool == "browser_open" and LOCAL_PAGE.search(summary)
                                              for _index, tool, summary in found))
    cited = []
    if kind == "lookup":
        seen = fetched_urls(trace)
        hosts = task.get("evidence_hosts") or []
        cited = [url.rstrip(".,;") for url in URL.findall(answer)]
        official = [url for url in cited if any(host in url for host in hosts)]
        measures["searched"] = int(any(tool in ("web_search", "fetch_url") for _i, tool, _s in found))
        measures["evidence_cited"] = int(any(_same_page(url, seen) for url in official))
        measures["reason_correct"] = int(any(re.search(pattern, answer, re.IGNORECASE)
                                             for pattern in task.get("answer_patterns") or []))
    end = trace.get("end") or {}
    measures["failed_run"] = int(end.get("reason") in ("failed", "crash", "server_hang") and not infrastructure(trace))
    usage = trace.get("usage") or {}
    measures["tool_calls"] = len(found)
    measures["wall_minutes"] = round((trace.get("wall_s") or 0) / 60, 1)
    measures["requests"] = usage.get("requests")
    measures["cost_usd"] = round(trace.get("cost") or 0, 4)
    details = {"end_state": end.get("reason"), "end_detail": end.get("detail") or "",
               "hidden": hidden.get("line", ""), "claimed": claimed, "cited": cited[:5],
               "answer": answer[-600:]}
    return measures, details


def infrastructure(trace):
    """A run lost to the provider (rate limits, outages), not to the agent or the app."""
    end = trace.get("end") or {}
    if end.get("reason") == "infrastructure":
        return True
    return end.get("reason") == "failed" and is_infrastructure(classify_error(end.get("detail")))
