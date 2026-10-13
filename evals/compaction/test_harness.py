"""CI-safe checks: grading rules, aggregation, and an offline end-to-end run.

The end-to-end test builds the `compaction_eval` example (debug) and drives it
through the real harness against a local scripted HTTP server, so no network or
model is involved. It measures plumbing only, never model quality.
"""

import http.server
import json
import os
import shutil
import subprocess
import tempfile
import threading
import unittest

from . import __main__ as harness
from .grade import grade, position
from .report import aggregate, spread, trial_rows, trial_summary

SUMMARY = ("Goal/constraints: keep the fixture facts. Confirmed findings: scripted checkpoint. "
           "Completed changes/tests: observed actions only. Unresolved work: finish the request. "
           "Next action: continue from recorded state.")


class GradeTests(unittest.TestCase):
    def test_correct_unknown_and_error(self):
        probe = {"expected": {"any_of": ["Token Bucket"]}}
        self.assertEqual(grade(probe, "We use a token   bucket."), "correct")
        self.assertEqual(grade(probe, "UNKNOWN"), "unknown")
        self.assertEqual(grade(probe, " unknown."), "unknown")
        self.assertEqual(grade(probe, None), "error")
        self.assertEqual(grade(probe, "a leaky thing"), "wrong")

    def test_regex_and_stale(self):
        probe = {"expected": {"regex": r"eu-central-1"}, "stale": [r"us-east-1"]}
        self.assertEqual(grade(probe, "Region eu-central-1"), "correct")
        self.assertEqual(grade(probe, "Region us-east-1"), "stale")
        self.assertEqual(grade(probe, "eu-central-1 (was us-east-1)"), "correct")
        self.assertEqual(grade(probe, "somewhere else"), "wrong")

    def test_unknown_is_not_substring_match(self):
        probe = {"expected": {"regex": r"^\W*no\b"}}
        self.assertEqual(grade(probe, "UNKNOWN"), "unknown")
        self.assertEqual(grade(probe, "No."), "correct")

    def test_decoys(self):
        probe = {"expect_unknown": True}
        self.assertEqual(grade(probe, "UNKNOWN"), "correct_unknown")
        self.assertEqual(grade(probe, "redis.internal"), "invented")

    def test_position_thirds(self):
        self.assertEqual([position(n, 30) for n in (1, 10, 11, 20, 21, 30)],
                         ["early", "early", "middle", "middle", "late", "late"])


class ReportTests(unittest.TestCase):
    def case(self):
        return {"name": "c", "records": 30, "probes": [
            {"id": "a", "category": "decision", "planted_at": 1, "expected": {"any_of": ["x"]}},
            {"id": "b", "category": "superseded", "planted_at": 25, "expected": {"any_of": ["new"]},
             "stale": ["old"]},
            {"id": "d", "category": "decoy", "planted_at": 1, "expect_unknown": True}]}

    def run_report(self, answers):
        chunk = {"installed": True, "duration_ms": 100, "tokens_before": 900, "tokens_after": 300,
                 "requests": 2}
        return {"answers": [{"id": k, "answer": v} for k, v in answers.items()], "chunks": [chunk]}

    def test_summary_counts(self):
        run = self.run_report({"a": "x", "b": "old value", "d": "made up"})
        rows = trial_rows(self.case(), run)
        summary = trial_summary(rows, [run])
        self.assertEqual(summary["retention"], 0.5)
        self.assertEqual(summary["stale_fact_rate"], 1.0)
        self.assertEqual(summary["invented_decoy_answers"], 1)
        self.assertEqual(summary["by_position"]["early"], 1.0)
        self.assertEqual(summary["by_position"]["late"], 0.0)
        self.assertEqual(summary["tokens_after_mean"], 300)

    def test_spread(self):
        self.assertEqual(spread([0.5, 1.0])["mean"], 0.75)
        self.assertEqual(spread([1.0])["sd"], 0.0)
        self.assertIsNone(spread([]))
        trial = {"retention": 0.5, "by_category": {}, "by_position": {}}
        self.assertEqual(aggregate([trial, trial])["retention"]["mean"], 0.5)


class Mock(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        system = body["messages"][0]["content"]
        text = SUMMARY if "You summarize agent context" in system else "UNKNOWN"
        payload = json.dumps({"id": "x", "object": "chat.completion", "choices": [
            {"index": 0, "message": {"role": "assistant", "content": text},
             "finish_reason": "stop"}]}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *args):
        pass


@unittest.skipUnless(shutil.which("cargo"), "cargo is required to build the eval example")
class EndToEndTests(unittest.TestCase):
    def test_harness_runs_the_real_example_against_a_scripted_server(self):
        subprocess.run(["cargo", "build", "-q", "-p", "themis-core", "--features", "eval",
                        "--example", "compaction_eval"], cwd=harness.ROOT, check=True)
        target = os.environ.get("CARGO_TARGET_DIR", os.path.join(harness.ROOT, "target"))
        runner = os.path.join(target, "debug", "examples", "compaction_eval" + harness.EXE)
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Mock)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        with tempfile.TemporaryDirectory() as tmp:
            out = os.path.join(tmp, "result.json")
            code = harness.main([
                "run", "--suite", "core", "--case", "narrative-lighthouse", "--trials", "2",
                "--model", "mock-model", "--budget", "24000", "--runner", runner,
                "--base-url", f"http://127.0.0.1:{server.server_address[1]}", "--out", out])
            self.assertEqual(code, 0)
            with open(out, encoding="utf-8") as handle:
                result = json.load(handle)
        self.assertEqual(result["trials_with_data"], 2)
        self.assertEqual(result["failed_runs"], [])
        self.assertEqual(result["aggregate"]["retention"]["mean"], 0.0)  # mock always says UNKNOWN
        self.assertEqual(result["aggregate"]["invented_decoy_answers"]["mean"], 0.0)
        self.assertEqual(result["aggregate"]["compactions_failed"]["mean"], 0.0)
        self.assertGreater(result["aggregate"]["tokens_before_mean"]["mean"],
                           result["aggregate"]["tokens_after_mean"]["mean"])
        self.assertIn("early", result["aggregate"]["by_position"])


class FixtureTests(unittest.TestCase):
    def test_every_core_case_is_well_formed(self):
        cases = harness.load_cases("core")
        self.assertGreaterEqual(len(cases), 8)
        kinds = {}
        for case in cases:
            kinds[case["kind"]] = kinds.get(case["kind"], 0) + 1
            self.assertEqual(case["chunks"], sorted(set(case["chunks"])))
            ids = [p["id"] for p in case["probes"]]
            self.assertEqual(len(ids), len(set(ids)))
            with open(os.path.join(case["directory"], "transcript.jsonl"), encoding="utf-8") as h:
                self.assertEqual(sum(1 for _ in h), case["records"])
            self.assertTrue(any(p["category"] == "decoy" for p in case["probes"]))
        self.assertGreaterEqual(kinds["coding"], 3)
        self.assertGreaterEqual(kinds["parked_topic"], 2)
        self.assertGreaterEqual(kinds["superseded"], 2)
        self.assertGreaterEqual(kinds["narrative"], 1)


if __name__ == "__main__":
    unittest.main()
