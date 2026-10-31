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
from . import behavior
from .grade import grade, matches, position
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
        self.assertEqual(grade(probe, "Unknown redis.internal"), "invented")
        for abstention in ("UNKNOWN - the conversation never says.", "It was not mentioned.",
                           "The context does not specify a hostname.", "No information is available."):
            self.assertEqual(grade(probe, abstention), "correct_unknown", abstention)

    def test_unknown_followed_by_a_value_is_not_an_abstention(self):
        probe = {"expected": {"any_of": ["t3.large"]}}
        self.assertEqual(grade(probe, "Unknown, probably t3.small"), "wrong")
        self.assertEqual(grade(probe, "UNKNOWN - the context does not say."), "unknown")
        self.assertEqual(grade(probe, "The size is not mentioned."), "unknown")

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
        self.assertEqual(summary["overall_retention"], 0.5)
        self.assertFalse(summary["tail_split_available"])
        self.assertIsNone(summary["summary_only_retention"])
        self.assertEqual(summary["stale_fact_rate"], 1.0)
        self.assertEqual(summary["invented_decoy_answers"], 1)
        self.assertEqual(summary["by_position"]["early"], 1.0)
        self.assertEqual(summary["by_position"]["late"], 0.0)
        self.assertEqual(summary["tokens_after_mean"], 300)

    def test_summary_only_headline_excludes_the_verbatim_tail(self):
        # "a" (record 1) was summarized and answered wrong; "b" (record 25) is verbatim and right.
        run = self.run_report({"a": "nope", "b": "new", "d": "UNKNOWN"})
        run["verbatim_records"] = [25]
        rows = trial_rows(self.case(), run)
        self.assertEqual([r["verbatim"] for r in rows], [False, True, False])
        summary = trial_summary(rows, [run])
        self.assertEqual(summary["summary_only_retention"], 0.0)
        self.assertEqual(summary["tail_retention"], 1.0)
        self.assertEqual(summary["overall_retention"], 0.5)
        self.assertEqual(summary["by_position"]["early"], 0.0)  # headline basis
        self.assertEqual(summary["by_position_overall"]["late"], 1.0)

    def test_probe_request_errors_are_counted_but_not_scored(self):
        run = self.run_report({"a": "x", "b": None, "d": "UNKNOWN"})
        run["answers"][1]["error"] = "Generic Error : Provider response was truncated"
        summary = trial_summary(trial_rows(self.case(), run), [run])
        self.assertEqual(summary["overall_retention"], 1.0)
        self.assertEqual(summary["probe_errors"], 1)

    def test_invalid_runs_are_excluded_and_reported(self):
        good = self.run_report({"a": "x", "b": "new", "d": "UNKNOWN"})
        bad = dict(self.run_report({"a": "x", "b": "new", "d": "UNKNOWN"}), valid=False)
        bad["chunks"] = [dict(bad["chunks"][0], installed=False, error="boom")]
        case = dict(self.case(), directory="")
        summary = harness.build_summary([case], {(0, "c"): good, (1, "c"): bad}, 2)
        self.assertEqual(summary["trials_with_data"], 1)
        self.assertEqual(summary["invalid_runs"], [{"trial": 1, "case": "c", "errors": ["boom"]}])

    def test_spread(self):
        self.assertEqual(spread([0.5, 1.0])["mean"], 0.75)
        self.assertEqual(spread([1.0])["sd"], 0.0)
        self.assertIsNone(spread([]))
        trial = {"by_category": {}, "by_position": {}, "by_category_overall": {},
                 "by_position_overall": {}}
        trial.update(summary_only_retention=0.5, tail_split_available=True)
        self.assertEqual(aggregate([trial, trial])["summary_only_retention"]["mean"], 0.5)


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
        runner = os.path.join(harness.target_dir(), "debug", "examples", "compaction_eval" + harness.EXE)
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
        self.assertEqual(result["aggregate"]["overall_retention"]["mean"], 0.0)  # mock says UNKNOWN
        self.assertTrue(result["aggregate"]["tail_split_available"])
        self.assertGreater(result["aggregate"]["summary_only_probes"]["mean"], 0)
        self.assertEqual(result["aggregate"]["invented_decoy_answers"]["mean"], 0.0)
        self.assertEqual(result["aggregate"]["compactions_failed"]["mean"], 0.0)
        self.assertGreater(result["aggregate"]["tokens_before_mean"]["mean"],
                           result["aggregate"]["tokens_after_mean"]["mean"])
        self.assertIn("early", result["aggregate"]["by_position"])


SCRIPTED_REPLIES = [  # (last user message contains, reply)
    ("capital of Australia", "Canberra."),
    ("ounces", "16 ounces."),
    ("necessary", "n-e-c-e-s-s-a-r-y."),
    ("Lisbon", "Lisbon: March 14 to 19, 1800 euro budget, peanut allergy, staying in Alfama."),
    ("rate limiter", "Free 60, paid 600, header X-RateLimit-Remaining, refill with the monotonic clock."),
    ("continue", "Continuing with round_cents in totals.py: switching to ROUND_HALF_UP."),
    ("go on", "Next: round_cents gets ROUND_HALF_UP in totals.py."),
    ("keep going", "Adding the regression test for round_cents."),
]


class BehaviorMock(http.server.BaseHTTPRequestHandler):
    """Scripted provider: answers by the last user message; `resume` makes greetings resume."""
    resume = False
    anchored = []

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        messages = body["messages"]
        if "You summarize agent context" in messages[0]["content"]:
            text = SUMMARY
        else:
            # The runtime's own note after an announced step isn't the person's message.
            person = [m for m in messages if m["role"] == "user" and "[Runtime note" not in str(m["content"])]
            last = person[-1]["content"] if person else messages[-1]["content"]
            if isinstance(last, list):
                last = json.dumps(last)
            type(self).anchored.append(any("Original exchange anchors" in str(m["content"]) for m in messages))
            text = next((r for k, r in SCRIPTED_REPLIES if k in last), "Hello! How can I help?")
            if type(self).resume and last.strip() in ("hi", "thanks!", "\u4f60\u597d"):
                text = "Continuing the lexer work: fixing scan_ident now."
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


class BehaviorGradeTests(unittest.TestCase):
    CASE = {"markers": r"(?i)lexer|scan_ident", "max_chars": 300}

    def test_greeting_must_be_short_toolless_and_not_resume(self):
        spec = {"id": "hi", "kind": "greeting"}
        ok = behavior.grade_turn(self.CASE, spec, {"reply": "Hi! What can I help with?", "tool_calls": []})
        self.assertTrue(ok["pass"])
        self.assertFalse(ok["resumed"])
        for turn in ({"reply": "Resuming the lexer fix.", "tool_calls": []},
                     {"reply": "Hello", "tool_calls": ["read_file: lexer.rs"]},
                     {"reply": "x" * 400, "tool_calls": []},
                     {"reply": "Hello", "tool_calls": [], "compactions": 1}):
            self.assertFalse(behavior.grade_turn(self.CASE, spec, turn)["pass"], turn)

    def test_unrelated_needs_the_answer(self):
        spec = {"id": "q", "kind": "unrelated", "must_match": [r"(?i)canberra"]}
        self.assertTrue(behavior.grade_turn(self.CASE, spec, {"reply": "Canberra", "tool_calls": []})["pass"])
        self.assertFalse(behavior.grade_turn(self.CASE, spec, {"reply": "Sydney", "tool_calls": []})["pass"])

    def test_return_scores_fact_share_and_continue_rejects_drift(self):
        spec = {"id": "r", "kind": "return", "facts": [r"March 14", r"1,?800"]}
        row = behavior.grade_turn(self.CASE, spec, {"reply": "March 14 only", "tool_calls": []})
        self.assertEqual((row["facts_found"], row["facts_total"], row["pass"]), (1, 2, False))
        spec = {"id": "c", "kind": "continue", "resume": r"round_cents", "wrong_topic": r"pancake"}
        self.assertTrue(behavior.grade_turn(self.CASE, spec, {"reply": "round_cents next", "tool_calls": []})["pass"])
        self.assertFalse(behavior.grade_turn(self.CASE, spec, {"reply": "round_cents and pancake", "tool_calls": []})["pass"])
        self.assertFalse(behavior.grade_turn(self.CASE, spec, {"reply": "What task?", "tool_calls": []})["pass"])

    def test_no_redo_classifies_the_first_call(self):
        spec = {"id": "n", "kind": "no_redo", "resume": r"(?i)ephemeris", "redo": r"(?i)data/vsop87|curl"}
        write = 'write_file: call `write_file` with {"file_path":"src/astro/ephemeris.js","content":"load(\'data/vsop87/VSOP87D.ear\')"}'
        reread = 'shell: call `shell` with {"command":"head","args":["data/vsop87/VSOP87D.ear"]}'
        look = 'shell: call `shell` with {"command":"ls","args":["src/astro"]}'
        row = behavior.grade_turn(self.CASE, spec, {"reply": "", "tool_calls": [write, reread]})
        self.assertEqual((row["first_call"], row["redo_calls"], row["pass"]), ("progress", 1, True))
        row = behavior.grade_turn(self.CASE, spec, {"reply": "", "tool_calls": [reread, write]})
        self.assertEqual((row["first_call"], row["pass"]), ("redo", False))
        row = behavior.grade_turn(self.CASE, spec, {"reply": "", "tool_calls": [look, write]})
        self.assertEqual((row["first_call"], row["pass"]), ("other", True))
        row = behavior.grade_turn(self.CASE, spec, {"reply": "Done.", "tool_calls": []})
        self.assertEqual((row["first_call"], row["pass"]), (None, False))

    def test_no_redo_case_counts_rereads_not_listings(self):
        case = next(c for c in harness.load_cases("behavior") if c["name"] == "behavior-no-redo")
        spec = case["behavior"]["turns"][0]
        self.assertTrue(os.path.isfile(os.path.join(case["directory"], "project", "data", "vsop87", "VSOP87D.ear")))
        for call, kind in (('list_dir: call `list_dir` with {"directory_path":"data/vsop87"}', "other"),
                           ('shell: call `shell` with {"command":"sh","args":["-c","ls data/vsop87; cat package.json"]}', "other"),
                           ('shell: call `shell` with {"command":"sh","args":["-c","head -20 data/vsop87/VSOP87D.ear"]}', "redo"),
                           ('read_file: call `read_file` with {"file_path":"data/vsop87/vsop87.txt"}', "redo"),
                           ('read_file: call `read_file` with {"file_path":"data/horizons_reference.json"}', "other")):
            self.assertEqual(behavior.classify_call(call, spec["resume"], spec["redo"]), kind, call)

    def test_no_redo_reports_whether_the_checkpoint_kept_the_facts(self):
        case = next(c for c in harness.load_cases("behavior") if c["name"] == "behavior-no-redo-chain")
        with open(os.path.join(case["directory"], "case.json"), encoding="utf-8") as handle:
            self.assertGreaterEqual(len(json.load(handle)["chunks"]), 5)
        call = 'shell: call `shell` with {"command":"sh","args":["-c","head -20 data/vsop87/VSOP87D.ear"]}'
        kept = {"turns": [{"id": "continue", "reply": "", "tool_calls": [call]}],
                "checkpoint": "Earlier context checkpoint: A*cos(B+C*T), T = (JDE-2451545)/365250, A amplitude"}
        lost = dict(kept, checkpoint="Earlier context checkpoint: parser written.")
        self.assertTrue(behavior.grade_run(case, kept)[0]["retained"])
        self.assertFalse(behavior.grade_run(case, lost)[0]["retained"])
        rows = behavior.grade_run(case, kept) + behavior.grade_run(case, lost)
        summary = behavior.summarize(rows)
        self.assertEqual((summary["facts_retained_rate"], summary["first_call_redo_rate_when_retained"]), (0.5, 1.0))

    def test_every_behavior_case_is_well_formed(self):
        cases = harness.load_cases("behavior")
        self.assertEqual(sorted(c["name"] for c in cases), [
            "behavior-continue", "behavior-greeting", "behavior-no-redo", "behavior-no-redo-chain",
            "behavior-topic-return", "behavior-unrelated"])
        for case in cases:
            ids = [t["id"] for t in case["behavior"]["turns"]]
            self.assertEqual(len(ids), len(set(ids)))
            self.assertTrue(all(t["kind"] in ("greeting", "unrelated", "return", "continue", "no_redo")
                                for t in case["behavior"]["turns"]))
        greeting = next(c for c in cases if c["name"] == "behavior-greeting")
        self.assertTrue(any(not t["message"].isascii() for t in greeting["behavior"]["turns"]))
        self.assertTrue(all(t["social"] for t in greeting["behavior"]["turns"]))


@unittest.skipUnless(shutil.which("cargo"), "cargo is required to build the eval example")
class BehaviorEndToEndTests(unittest.TestCase):
    """Plumbing only: a scripted provider, never a quality claim about a model."""

    def run_behavior(self, resume, anchors):
        subprocess.run(["cargo", "build", "-q", "-p", "themis-core", "--features", "eval",
                        "--example", "behavior_eval"], cwd=harness.ROOT, check=True)
        runner = os.path.join(harness.target_dir(), "debug", "examples", "behavior_eval" + harness.EXE)
        BehaviorMock.resume = resume
        BehaviorMock.anchored = []
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), BehaviorMock)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        with tempfile.TemporaryDirectory() as tmp:
            out = os.path.join(tmp, "result.json")
            code = harness.main([
                "behavior", "--trials", "1", "--model", "mock-model", "--budget", "24000",
                "--anchors", anchors, "--runner", runner, "--jobs", "2",
                "--base-url", f"http://127.0.0.1:{server.server_address[1]}", "--out", out])
            self.assertEqual(code, 0)
            with open(out, encoding="utf-8") as handle:
                return json.load(handle)

    def test_good_model_passes_every_behavior_turn_without_tools_or_compaction(self):
        result = self.run_behavior(resume=False, anchors="new")
        summary = result["summary"]
        self.assertEqual(result["failed_runs"], [])
        self.assertEqual(summary["greeting_tool_calls"], 0)
        self.assertEqual(summary["greeting_compactions"], 0)
        self.assertEqual(summary["greeting_pass_rate"], 1.0)
        self.assertEqual(summary["unwanted_resume_rate"], 0.0)
        self.assertEqual(summary["topic_return_fact_retention"], 1.0)
        self.assertEqual(summary["continue_resume_rate"], 1.0)
        self.assertEqual(summary["errors"], 0)

    def test_resuming_model_is_counted_and_old_anchors_are_injected_for_greetings(self):
        result = self.run_behavior(resume=True, anchors="old")
        self.assertEqual(result["summary"]["greeting_pass_rate"], 0.25)  # only the unscripted "howdy"
        self.assertGreater(result["summary"]["unwanted_resume_rate"], 0.0)
        self.assertTrue(any(BehaviorMock.anchored))
        new = self.run_behavior(resume=False, anchors="new")
        greeting_requests = [t for d in new["details"] if d["case"] == "behavior-greeting" for t in d["turns"]]
        self.assertEqual(len(greeting_requests), 4)
        # No anchor message reaches a social turn; the other cases may still carry one.
        self.assertTrue(BehaviorMock.anchored.count(False) >= 4)


KNOWN_WRONG = {
    # (case, probe): answers that must never be graded correct.
    ("coding-csv-migration", "decimal"): ["A period", "Dot, as in 12.50", "A semicolon", "tab"],
    ("coding-csv-migration", "cause"): ["A namespace collision", "Encoding mismatch"],
    ("superseded-deploy-plan", "why_eu"): [
        "To use a neutral queue", "Because Europe is nice", "Lower cost", "Latency to customers"],
    ("superseded-deploy-plan", "redis"): [
        "Yes, Redis will be used", "Note that Redis is fine", "Nothing prevents it"],
    ("superseded-deploy-plan", "analytics"): [
        "Yes, it will be migrated", "Nothing was said", "It cannot be migrated"],
    ("constraints-api-design", "deps"): ["Yes, they are allowed", "Note: fine", "Nothing forbids it"],
    ("constraints-api-design", "audit"): ["Yes, it is included", "Note the audit log changes",
                                          "Nothing else"],
    ("coding-rust-parser", "no_regex"): ["Yes, it is acceptable", "Note that regex helps",
                                         "Nothing stops it", "It cannot hurt"],
    ("parked-budget-recipe", "test_fail"): ["The test was wrong somehow", "Wrong import"],
    ("constraints-api-design", "why_cursor"): ["It would break the API", "Offsets are fine"],
}
KNOWN_RIGHT = {
    ("coding-csv-migration", "decimal"): "A comma",
    ("superseded-deploy-plan", "why_eu"): "Legal required customer data to stay in the EU",
    ("superseded-deploy-plan", "redis"): "No, Redis is not to be used",
    ("superseded-deploy-plan", "analytics"): "No, it is not being migrated this quarter",
    ("constraints-api-design", "deps"): "No new dependencies",
    ("constraints-api-design", "audit"): "No, it is out of scope",
    ("coding-rust-parser", "no_regex"): "No, do not add it",
}


class FixtureTests(unittest.TestCase):
    def test_fixed_probes_reject_known_wrong_answers(self):
        probes = {(c["name"], p["id"]): p for c in harness.load_cases("core") for p in c["probes"]}
        for key, answers in KNOWN_WRONG.items():
            for answer in answers:
                self.assertNotEqual(grade(probes[key], answer), "correct", (key, answer))
        for key, answer in KNOWN_RIGHT.items():
            self.assertEqual(grade(probes[key], answer), "correct", (key, answer))

    def test_planted_record_states_the_fact(self):
        for case in harness.load_cases("core"):
            with open(os.path.join(case["directory"], "transcript.jsonl"), encoding="utf-8") as h:
                lines = h.read().splitlines()
            for probe in case["probes"]:
                if probe.get("expect_unknown"):
                    continue
                spec = {"regex": probe["needle"]} if probe.get("needle") else probe["expected"]
                self.assertTrue(matches(spec, lines[probe["planted_at"] - 1]),
                                (case["name"], probe["id"]))

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
