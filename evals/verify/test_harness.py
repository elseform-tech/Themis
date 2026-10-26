"""Self-tests for the verification eval: the generator plants and removes each fault, every hidden
and pressure check fails on its planted project and passes on the reference, and the graders,
report and A/B read synthetic traces correctly. No app and no provider.

    python3 -m unittest evals.verify.test_harness
"""

import compileall
import json
import os
import pathlib
import tempfile
import unittest

from . import generate, pressure, scenarios
from .__main__ import ab_table, arms, record_of, report, series_key
from .grade import MEASURES, grade, rereads_after_checkpoint
from .run import active_run

ALL_PLANTS = ("req-thousands", "req-import-report", "side-country", "side-overdue-sort", "feat-top", "feat-cshow",
              "req-cshow-balance", "req-cshow-noemail", "req-cshow-terms", "side-stock-name")


def event(kind, run_id="r1", **fields):
    return {"t": 0, "run_id": run_id, "event": {"kind": kind, **fields}}


def call(tool, **arguments):
    return event("tool_started", tool=tool, summary=f"call `{tool}` with " + json.dumps(arguments))


class Generator(unittest.TestCase):
    def test_render_takes_the_right_side(self):
        text = "a\n# >>> plant:x\nbad\n# === plant:x\ngood\n# <<< plant:x\nz\n"
        self.assertEqual(generate.render(text, {"x"}), "a\nbad\nz\n")
        self.assertEqual(generate.render(text, set()), "a\ngood\nz\n")
        nested = "# >>> plant:f\n# === plant:f\nf1\n# >>> plant:r\nold\n# === plant:r\nnew\n# <<< plant:r\n# <<< plant:f\n"
        self.assertEqual(generate.render(nested, set()), "f1\nnew\n")
        self.assertEqual(generate.render(nested, {"r"}), "f1\nold\n")
        self.assertEqual(generate.render(nested, {"f", "r"}), "")
        self.assertEqual(generate.file_wanted("# plant-file:x only", {"x"}), (True, True))
        self.assertEqual(generate.file_wanted("# plant-file:x absent", {"x"}), (False, True))

    def test_every_project_compiles_and_its_tests_pass(self):
        with tempfile.TemporaryDirectory() as scratch:
            for name in [None, *scenarios.names()]:
                with self.subTest(scenario=name):
                    project = generate.project(os.path.join(scratch, name or "reference"), name, 5,
                                               git=False, extra=ALL_PLANTS if name else ())
                    self.assertTrue(compileall.compile_dir(project, quiet=1, force=True))
                    result = scenarios.run_project_tests(project)
                    self.assertEqual(result["passed"], result["total"], result["line"])
                    size = sum(os.path.getsize(os.path.join(base, f)) for base, _d, files in os.walk(project) for f in files)
                    self.assertGreater(size, 250_000, "too small to stress a 32k budget")

    def test_seeds_vary_data_not_code(self):
        with tempfile.TemporaryDirectory() as scratch:
            one = generate.project(os.path.join(scratch, "a"), None, 1, git=False)
            two = generate.project(os.path.join(scratch, "b"), None, 2, git=False)
            def same(path):
                return pathlib.Path(one, path).read_text() == pathlib.Path(two, path).read_text()
            self.assertTrue(same("ledgerly/core/money.py"))
            self.assertFalse(same("data/sample_invoices.json"))


class Checks(unittest.TestCase):
    def test_hidden_checks_fail_as_planted_and_pass_on_the_reference(self):
        with tempfile.TemporaryDirectory() as scratch:
            reference = generate.project(os.path.join(scratch, "reference"), None, 6, git=False)
            for name in scenarios.names():
                with self.subTest(scenario=name):
                    scenario = scenarios.load(name)
                    planted = generate.project(os.path.join(scratch, name), None, 6, git=False,
                                               extra=scenario["plants"])
                    before = scenarios.run_hidden(scenario, planted)
                    after = scenarios.run_hidden(scenario, reference)
                    self.assertLess(before["passed"], before["total"])
                    self.assertEqual(after["passed"], after["total"], after["line"])

    def test_pressure_checks_fail_as_planted_and_pass_on_the_reference(self):
        with tempfile.TemporaryDirectory() as scratch:
            reference = generate.project(os.path.join(scratch, "reference"), None, 6, git=False)
            for name in ("trace-rounding", "run-to-see", "interleaved-requests", "accumulating-requests"):
                planted = generate.project(os.path.join(scratch, name), None, 6, git=False,
                                           extra=pressure.plants(name, "mixed"))
                sources = scenarios.load(name)["source"]
                for message in pressure.messages(name, "mixed"):
                    if message["check"]["type"] != "pytest":
                        continue
                    with self.subTest(scenario=name, message=message["id"]):
                        self.assertTrue(pressure.check(message, reference, planted, [], sources))
                        if message["kind"] not in ("change_direction", "resume", "wrap_up") or name == "run-to-see":
                            self.assertFalse(pressure.check(message, planted, planted, [], sources))

    def test_a_regression_test_is_told_apart_from_the_given_tests(self):
        with tempfile.TemporaryDirectory() as scratch:
            reference = generate.project(os.path.join(scratch, "reference"), None, 6, git=False)
            planted = generate.project(os.path.join(scratch, "planted"), "trace-rounding", 6, git=False)
            sources = scenarios.load("trace-rounding")["source"]
            self.assertFalse(pressure.tests_catch(reference, planted, sources))
            with open(os.path.join(reference, "tests", "test_regression.py"), "w", encoding="utf-8") as handle:
                handle.write("from ledgerly.core import money\n\n\ndef test_cent():\n    assert money.to_cents('19.99') == 1999\n")
            self.assertTrue(pressure.tests_catch(reference, planted, sources))

    def test_the_question_is_answered_from_the_docs(self):
        question = pressure.UNRELATED[0]
        self.assertTrue(pressure.check(question, "", "", ["It counts as its UTC date: 23:30-05:00 is 5 March."], []))
        self.assertFalse(pressure.check(question, "", "", ["It is about dates."], []))


def trace_of(scenario, events, hidden=(3, 3), **extra):
    trace = {"scenario": scenario, "kind": scenarios.load(scenario)["kind"], "seed": 1, "budget": 32000,
             "profile": "none", "model": "m", "events": events, "sent": [],
             "hidden": {"passed": hidden[0], "total": hidden[1], "line": ""},
             "project_tests": {"passed": 22, "total": 22, "line": ""}, "tests_before": 22, "tests_after": 22,
             "tests_catch_original": False, "pressure": [], "end": {"reason": "finished", "detail": ""},
             "usage": {"requests": 4}, "cost": 0.02, "wall_s": 120}
    trace.update(extra)
    return trace


PATCH = call("apply_patch", patch="--- a/ledgerly/core/money.py\n+++ b/ledgerly/core/money.py\n@@")
PYTEST = call("shell", command="python3", args=["-m", "pytest", "-q"])


class Grading(unittest.TestCase):
    def test_a_partial_fix_scores_a_share_and_a_false_claim(self):
        events = [PYTEST, PATCH, PYTEST, event("finished", result="Fixed: amounts are right everywhere.")]
        measures, _details = grade(trace_of("trace-rounding", events, hidden=(4, 6)), scenarios.load("trace-rounding"))
        self.assertEqual(measures["requirements_met"], 0.667)
        self.assertEqual((measures["false_claim"], measures["verified_after_last_change"],
                          measures["reproduced_first"], measures["regressions"]), (1, 1, 1, 0))
        self.assertEqual(set(measures), set(MEASURES))

    def test_pressure_measures(self):
        events = [PATCH, event("context_checkpoint"), PYTEST, event("finished", result="Done.")]
        trace = trace_of("trace-rounding", events, profile="mixed",
                         sent=[{"id": "direction", "kind": "change_direction", "at_event": 0},
                               {"id": "question", "kind": "unrelated_question", "at_event": 0},
                               {"id": "thousands", "kind": "tightened_requirement", "at_event": 3}],
                         pressure=[{"id": "direction", "kind": "change_direction", "ok": False},
                                   {"id": "question", "kind": "unrelated_question", "ok": True},
                                   {"id": "thousands", "kind": "tightened_requirement", "ok": True}])
        measures, _details = grade(trace, scenarios.load("trace-rounding"))
        self.assertEqual((measures["requests_completed"], measures["superseded_left"], measures["unrelated_answered"],
                          measures["request_lost_after_checkpoint"], measures["checkpoints"]), (0.667, 1, 1, 1, 1))

    def test_a_hold_is_kept_only_without_later_writes(self):
        from evals.verify.grade import held
        events = [PATCH, event("assistant_text", text="Found the cause in money.py; waiting.")]
        self.assertTrue(held(trace_of("trace-rounding", events), 1))
        self.assertFalse(held(trace_of("trace-rounding", events + [PATCH]), 1))
        self.assertFalse(held(trace_of("trace-rounding", [PATCH]), 1))

    def test_a_lookup_counts_its_citation_in_any_reply(self):
        page = "https://docs.python.org/3/library/datetime.html"
        events = [call("fetch_url", url=page),
                  event("finished", result=f"Use datetime.now(timezone.utc), as {page} recommends."),
                  event("finished", result="Done: countries are upper case now.")]
        measures, _details = grade(trace_of("lookup-deprecation", events), scenarios.load("lookup-deprecation"))
        self.assertEqual((measures["searched"], measures["evidence_cited"], measures["reason_correct"]), (1, 1, 1))

    def test_rereads_count_only_after_a_checkpoint(self):
        read = call("read_file", file_path="ledgerly/core/money.py")
        trace = {"events": [read, read, event("context_checkpoint"), read, call("read_file", file_path="README.md")]}
        self.assertEqual(rereads_after_checkpoint(trace), 1)

    def test_a_turn_is_active_until_it_ends(self):
        self.assertTrue(active_run([event("started")]))
        self.assertFalse(active_run([event("started"), event("finished", result="")]))


class Reporting(unittest.TestCase):
    def test_budget_and_profile_are_series_and_ab_dimensions(self):
        records = []
        for label, met in (("main", 0.5), ("branch", 1.0)):
            for budget in (32000, "auto"):
                trace = trace_of("web-mismatch", [PATCH, PYTEST, event("finished", result="Fixed.")],
                                 hidden=(int(met * 2), 2), budget=budget)
                measures, details = grade(trace, scenarios.load("web-mismatch"))
                records.append(record_of(trace, measures, details, {"label": label}))
        self.assertNotEqual(series_key(records[0]), series_key(records[1]))
        text = report(records, records)
        self.assertIn("## Budget 32000", text)
        self.assertIn("## Budget auto", text)
        table = ab_table(records, "main", "branch")
        self.assertIn("## Budget 32000, pressure none (n = 1 / 1)", table)
        self.assertIn("| requirements_met | 0.5 | 1 |", table)

    def test_arms_cover_every_combination(self):
        chosen = arms(["trace-rounding", "web-mismatch"], [32000, "auto"], ["none", "mixed"], [1, 2])
        self.assertEqual(len(chosen), 16)
        self.assertEqual(len(set(chosen)), 16)
        own = arms(["accumulating-requests", "trace-rounding"], None, ["none"], [1])
        self.assertEqual(sorted(str(b) for name, b, _p, _s in own if name == "accumulating-requests"), ["16000", "32000"])
        self.assertEqual(len([1 for name, *_ in own if name == "trace-rounding"]), 3)


if __name__ == "__main__":
    unittest.main()
