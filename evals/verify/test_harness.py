"""Self-tests for the verification eval: the hidden checks tell a broken project from a fixed one,
and the graders read synthetic traces correctly. No app and no provider.

    python3 -m unittest evals.verify.test_harness
"""

import json
import os
import shutil
import tempfile
import unittest

from evals.kit import trend as kit_trend

from . import tasks
from .__main__ import record_of, report, series_key
from .grade import MEASURES, claims_success, grade

FEATURE_FIX = '''import re

PATTERN = re.compile(r"\\s*(?:(\\d+)\\s*h)?\\s*(?:(\\d+)\\s*m)?\\s*(?:(\\d+)\\s*s)?\\s*", re.IGNORECASE)


def parse_duration(text):
    match = PATTERN.fullmatch(text)
    if not match or all(group is None for group in match.groups()):
        raise ValueError(text)
    hours, minutes, seconds = (int(group or 0) for group in match.groups())
    return hours * 3600 + minutes * 60 + seconds
'''
LOOKUP_FIX = '''from pathlib import Path

SIGNATURES = ((b"\\x89PNG\\r\\n\\x1a\\n", "png"), (b"\\xff\\xd8\\xff", "jpeg"), (b"GIF87a", "gif"), (b"GIF89a", "gif"))


def kind(path):
    head = Path(path).read_bytes()[:16]
    return next((name for magic, name in SIGNATURES if head.startswith(magic)), None)
'''
# task -> (file, fix): a correct fix the hidden check must accept.
REFERENCE = {
    "bug-discount": ("billing/invoice.py",
                     lambda text: text.replace("subtotal * tax_percent / 100", "discounted * tax_percent / 100")),
    "feature-duration": ("timeparse/duration.py", lambda _text: FEATURE_FIX),
    "web-clear": ("app.js", lambda text: text.replace("clear-done", "clear-completed")),
    "lookup-imghdr": ("tools/thumbs.py", lambda _text: LOOKUP_FIX),
}
REGRESSION_TEST = '''from billing.invoice import total_due


def test_tax_on_discounted_amount():
    assert total_due([(1250, 2), (500, 1)], discount_percent=10, tax_percent=20) == 3240
'''


def fixed_copy(name, scratch):
    task = tasks.load(name)
    project = os.path.join(scratch, name)
    shutil.copytree(task["project"], project)
    path, fix = REFERENCE[name]
    full = os.path.join(project, path)
    with open(full, encoding="utf-8") as handle:
        text = handle.read()
    with open(full, "w", encoding="utf-8") as handle:
        handle.write(fix(text))
    return task, project


def shell(command, *args):
    return {"kind": "tool_started", "tool": "shell",
            "summary": "call `shell` with " + json.dumps({"command": command, "args": list(args)})}


def event(item, t=0.0):
    return {"t": t, "run_id": "r1", "event": item}


def trace_of(task, events, answer, passed, **extra):
    trace = {"task": task, "kind": tasks.load(task)["kind"], "rep": 1, "model": "m",
             "events": [event(item, i) for i, item in enumerate(events)], "answer": answer,
             "hidden": {"passed": passed, "line": "ok" if passed else "1 failed"},
             "end": {"reason": "finished", "detail": ""}, "usage": {"requests": 3}, "cost": 0.01,
             "wall_s": 60}
    trace.update(extra)
    return trace


CHANGED = {"kind": "files_changed", "files": [{"path": "billing/invoice.py"}]}


class HiddenChecks(unittest.TestCase):
    def test_every_task_fails_as_given_and_passes_when_fixed(self):
        with tempfile.TemporaryDirectory() as scratch:
            for name in tasks.names():
                with self.subTest(task=name):
                    task = tasks.load(name)
                    self.assertFalse(tasks.run_hidden(task, task["project"])[0])
                    _task, project = fixed_copy(name, scratch)
                    passed, line = tasks.run_hidden(task, project)
                    self.assertTrue(passed, line)

    def test_a_regression_test_is_told_apart_from_the_given_tests(self):
        with tempfile.TemporaryDirectory() as scratch:
            task, project = fixed_copy("bug-discount", scratch)
            self.assertFalse(tasks.tests_catch_original(task, project))
            with open(os.path.join(project, "tests", "test_regression.py"), "w", encoding="utf-8") as handle:
                handle.write(REGRESSION_TEST)
            self.assertTrue(tasks.tests_catch_original(task, project))
            self.assertEqual(tasks.test_count(project, "tests"), 5)


class Grading(unittest.TestCase):
    def test_a_reproduced_fixed_and_rechecked_bug(self):
        events = [shell("python3", "-m", "pytest", "-q"), CHANGED, shell("python3", "-m", "pytest", "-q")]
        trace = trace_of("bug-discount", events, "Fixed: tax now applies to the discounted amount; tests pass.",
                         True, tests_before=4, tests_after=5, tests_catch_original=True)
        measures, details = grade(trace, tasks.load("bug-discount"))
        self.assertEqual((measures["correct"], measures["verified_after_last_change"], measures["unverified_claim"],
                          measures["false_claim"], measures["reproduced_first"], measures["test_catches_bug"],
                          measures["tests_added"]), (1, 1, 0, 0, 1, 1, 1))
        self.assertIsNone(measures["browser_checked"])
        self.assertTrue(details["claimed"])

    def test_a_claim_without_a_check_after_the_last_change(self):
        events = [CHANGED, {"kind": "tool_started", "tool": "read_file", "summary": "call `read_file`"}]
        trace = trace_of("bug-discount", events, "Done — the total is fixed.", False,
                         tests_before=4, tests_after=4, tests_catch_original=False)
        measures, _details = grade(trace, tasks.load("bug-discount"))
        self.assertEqual((measures["verified_after_last_change"], measures["unverified_claim"],
                          measures["false_claim"], measures["reproduced_first"], measures["tests_added"]),
                         (0, 1, 1, 0, 0))

    def test_claims_read_negations(self):
        self.assertTrue(claims_success("Implemented and all tests pass."))
        self.assertFalse(claims_success("I could not get it working yet."))
        self.assertFalse(claims_success("Here is what I found in the code."))

    def test_the_browser_counts_only_on_a_local_page(self):
        changed = {"kind": "files_changed", "files": [{"path": "app.js"}]}
        local = {"kind": "tool_started", "tool": "browser_open",
                 "summary": 'call `browser_open` with {"url":"http://127.0.0.1:8000/index.html"}'}
        remote = {"kind": "tool_started", "tool": "browser_open",
                  "summary": 'call `browser_open` with {"url":"https://example.com"}'}
        for events, expected in (([local, changed, local], (1, 1, 1)), ([remote, changed], (0, 0, 0))):
            measures, _details = grade(trace_of("web-clear", events, "Fixed the button.", True), tasks.load("web-clear"))
            self.assertEqual((measures["browser_checked"], measures["reproduced_first"],
                              measures["verified_after_last_change"]), expected)

    def test_evidence_must_be_a_page_the_agent_opened(self):
        page = "https://docs.python.org/3/library/imghdr.html"
        searched = {"kind": "tool_finished", "tool": "web_search", "output": json.dumps({"results": [{"url": page}]})}
        start = {"kind": "tool_started", "tool": "web_search", "summary": 'call `web_search` with {"query":"imghdr"}'}
        answer = f"imghdr was removed in Python 3.13 (PEP 594). Source: {page}"
        task = tasks.load("lookup-imghdr")
        measures, _details = grade(trace_of("lookup-imghdr", [start, searched], answer, True), task)
        self.assertEqual((measures["searched"], measures["evidence_cited"], measures["reason_correct"]), (1, 1, 1))
        unseen = grade(trace_of("lookup-imghdr", [], answer, True), task)[0]
        self.assertEqual((unseen["searched"], unseen["evidence_cited"]), (0, 0))

    def test_a_record_reports_against_its_series(self):
        trace = trace_of("feature-duration", [CHANGED], "Implemented.", True, tests_before=2, tests_after=2)
        measures, details = grade(trace, tasks.load("feature-duration"))
        record = record_of(trace, measures, details, {"commit": "abc"})
        self.assertEqual(series_key(record), ("m", "feature-duration"))
        self.assertEqual(set(record["measures"]), set(MEASURES))
        text = report([record], [record])
        self.assertIn("feature-duration (feature)", text)
        self.assertIn("| correctness | correct | model | 1 |", text)
        self.assertEqual(kit_trend.flags([record], record, MEASURES, series_key), {})


if __name__ == "__main__":
    unittest.main()
