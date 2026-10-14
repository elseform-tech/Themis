"""CI-safe checks for the long-form suite: generator, grader, citations, exclusion, offline e2e.

Nothing here calls a model. The end-to-end test builds the `longform_eval` example (debug) and
drives it through the real harness against a scripted local HTTP provider, so it measures
plumbing only, never model quality.
"""

import contextlib
import http.server
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest

from . import __main__ as harness
from . import gen_cases
from .grade import grade
from .kit import anchored
from .report import aggregate, classify_error, trial_summary
from .validate import context_location, parse_citations, record_text, validate

SUMMARY = ("Goal/constraints: keep the fixture facts. Confirmed findings: scripted checkpoint. "
           "Completed changes/tests: observed actions only. Unresolved work: finish the request. "
           "Next action: continue from recorded state.")


def read_normalized(path):
    with open(path, "rb") as handle:
        return handle.read().replace(b"\r\n", b"\n")


class GeneratorTests(unittest.TestCase):
    def test_committed_cases_match_a_fresh_generation(self):
        with tempfile.TemporaryDirectory() as scratch:
            original = gen_cases.CASES
            gen_cases.CASES = scratch
            try:
                with contextlib.redirect_stdout(io.StringIO()):
                    gen_cases.main()
            finally:
                gen_cases.CASES = original
            for name in sorted(os.listdir(scratch)):
                for part in ("transcript.jsonl", "probes.json", "case.json"):
                    fresh, committed = (read_normalized(os.path.join(root, name, part)) for root in (scratch, original))
                    self.assertEqual(fresh, committed, f"{name}/{part} is stale; run python3 -m evals.longform.gen_cases")

    def test_case_shape(self):
        cases = harness.load_cases()
        self.assertEqual(len(cases), 6)
        self.assertEqual(sorted(c["kind"] for c in cases), ["code"] * 3 + ["narrative"] * 3)
        for case in cases:
            with self.subTest(case["name"]):
                self.assertGreaterEqual(len(case["chunks"]), 3)
                self.assertGreaterEqual(case["material_bytes"], 60_000)
                decoys = [p for p in case["probes"] if p["category"] == "decoy"]
                quotes = [p for p in case["probes"] if p["category"] == "quote"]
                self.assertIn(len(decoys), (1, 2))
                self.assertIn(len(quotes), (1, 2))
                self.assertTrue(8 <= len(case["probes"]) - len(decoys) - len(quotes) <= 12)
                ids = [p["id"] for p in case["probes"]]
                self.assertEqual(len(ids), len(set(ids)))

    def test_planted_facts_appear_only_in_their_records(self):
        for case in harness.load_cases():
            with open(os.path.join(case["directory"], "transcript.jsonl"), encoding="utf-8") as handle:
                text = [json.dumps(json.loads(line), ensure_ascii=False) for line in handle]
            for probe in case["probes"]:
                if probe["category"] == "decoy":
                    for pattern in probe["absent"]:
                        self.assertFalse(any(re.search(pattern, t, re.I) for t in text), probe["id"])
                else:
                    self.assertTrue(probe["fact_records"], probe["id"])
                    self.assertTrue(re.search(probe["expected"]["regex"], probe["answer_text"], re.I | re.S),
                                    f"{probe['id']}: the literal answer does not satisfy its own pattern")


class GradeTests(unittest.TestCase):
    def test_numbers_are_anchored(self):
        probe = {"expected": {"regex": anchored("4096")}}
        self.assertEqual(grade(probe, "The limit is 4096 characters."), "correct")
        for wrong in ("40960", "It is 14096.", "about 4096.5", "v4096x", "UNKNOWN", "the limit is 2048"):
            self.assertNotEqual(grade(probe, wrong), "correct", wrong)

    def test_citation_lines_do_not_count_as_the_answer(self):
        probe = {"expected": {"regex": anchored("790")}}
        self.assertNotEqual(grade(probe, "I do not know.\nCitation: snapshot-1.jsonl record 790"), "correct")

    def test_quotes_need_exact_wording(self):
        probe = {"exact": "Brass lantern slate", "expected": {"regex": anchored("Brass lantern slate")}}
        self.assertEqual(grade(probe, 'She said: "Brass  lantern slate".'), "correct")
        self.assertEqual(grade(probe, "brass lantern slate"), "wrong")  # case matters
        self.assertEqual(grade(probe, "Brass slate lantern"), "wrong")
        self.assertEqual(grade(probe, "UNKNOWN"), "unknown")

    def test_rambling_dumps_that_contain_the_value_do_not_pass(self):
        probe = {"expected": {"regex": anchored("8752")}}
        self.assertEqual(grade(probe, "right: 8752"), "correct")
        self.assertEqual(grade(probe, "Summary of all shards. " * 60 + "right: 8752"), "wrong")

    def test_stale_value_first_is_stale(self):
        probe = {"expected": {"regex": anchored("361")}, "stale": [anchored("790")]}
        self.assertEqual(grade(probe, "361 guineas"), "correct")
        self.assertEqual(grade(probe, "790 guineas"), "stale")
        self.assertEqual(grade(probe, "790 guineas, corrected to 361"), "stale")
        self.assertEqual(grade(probe, "361 (it was 790 before)"), "correct")
        self.assertEqual(grade(probe, "500"), "wrong")

    def test_decoys(self):
        probe = {"expect_unknown": True}
        self.assertEqual(grade(probe, "UNKNOWN"), "correct_unknown")
        self.assertEqual(grade(probe, "The original material does not say."), "correct_unknown")
        self.assertEqual(grade(probe, "Rover"), "invented")
        self.assertEqual(grade(probe, None), "error")

    def test_hedged_answers_are_wrong(self):
        probe = {"question": "What did X count?", "expected": {"regex": anchored("5013")}}
        pool = ["5013", "6024", "brass lantern"]
        self.assertEqual(grade(probe, "5013", pool), "correct")
        self.assertEqual(grade(probe, "It was 5013 or 6024.", pool), "hedged")
        self.assertEqual(grade(probe, "5013, plus a brass lantern", pool), "hedged")
        self.assertEqual(grade({**probe, "question": "Did 6024 matter? What did X count?"}, "Not 6024: 5013", pool), "correct")
        stale = {**probe, "stale": [anchored("6024")]}
        self.assertEqual(grade(stale, "5013 (it was 6024 before)", pool), "correct")

    def test_abstentions_must_lead_or_name_no_candidate(self):
        probe = {"expect_unknown": True}
        for invented in ("The limit was 4096 (not mentioned elsewhere)", "The limit was X (not mentioned elsewhere)",
                         "It is 4096, though the log does not say more.", "Rover. The material never mentions a dog."):
            self.assertEqual(grade(probe, invented), "invented", invented)

    def test_prose_abstentions_on_decoys(self):
        probe = {"expect_unknown": True}
        for abstention in ("The search returned no matches for \"tavern\" in the saved originals.",
                           "UNKNOWN\n\nI searched the snapshot and found nothing.", "No.",
                           "I could not find any mention of that."):
            self.assertEqual(grade(probe, abstention), "correct_unknown", abstention)
        self.assertEqual(grade(probe, "Session 5 is complete and the bug is fixed."), "invented")

    def test_failed_requests_are_errors_but_unfinished_runs_are_wrong(self):
        self.assertIsNone(harness.stored_answer({}))
        self.assertIsNone(harness.stored_answer({"answer": "", "error": "LLM request failed: 429"}))
        self.assertEqual(harness.stored_answer({"answer": "", "error": "Repeated unchanged tool results stopped the run."}), "")
        self.assertEqual(harness.stored_answer({"answer": "x", "error": "y"}), "x")
        self.assertEqual(grade({"expected": {"regex": "x"}}, ""), "wrong")

    def test_known_wrong_answers_for_every_committed_probe(self):
        for case in harness.load_cases():
            for probe in case["probes"]:
                if probe["category"] == "decoy":
                    self.assertEqual(grade(probe, "A confident invented answer 12345"), "invented")
                else:
                    self.assertEqual(grade(probe, probe["answer_text"], case["tokens"]), "correct", probe["id"])
                    for wrong in ("UNKNOWN", "12345", "I could not find it."):
                        self.assertNotIn(grade(probe, wrong, case["tokens"]), ("correct",), (probe["id"], wrong))
                    # Two planted values (the right one plus another unique token) is a hedge.
                    other = next(t for t in case["tokens"] if t not in probe["question"].lower()
                                 and not re.search(probe["expected"]["regex"], t, re.I))
                    self.assertEqual(grade(probe, f"{probe['answer_text']} or {other}", case["tokens"]), "hedged", probe["id"])


class CitationTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.dir)
        records = [
            {"role": "User", "message_type": "Text", "content": "chapter text: the brass astrolabe was hidden", "evidence_kind": "original"},
            {"role": "Assistant", "message_type": "Text", "content": "the brass astrolabe note", "evidence_kind": "navigation"},
            {"role": "Tool", "message_type": {"ToolResult": [
                {"id": "a", "function": {"name": "grep", "arguments": "brass astrolabe copy"}, "evidence_kind": "recovery"},
                {"id": "b", "function": {"name": "shell", "arguments": "build failed at lib.rs:812:17"}, "evidence_kind": "original"}]},
             "content": "", "evidence_kind": "mixed"},
        ]
        with open(os.path.join(self.dir, "snapshot-111.jsonl"), "w", encoding="utf-8") as handle:
            handle.write("\n".join(json.dumps(r) for r in records) + "\n")
        self.probe = {"evidence": anchored("brass astrolabe")}

    def test_parse_forms(self):
        self.assertEqual(parse_citations("Citation: .themis/context/eval-thread/snapshot-111.jsonl record 3"),
                         [("snapshot-111.jsonl", 3)])
        self.assertEqual(parse_citations("snapshot-5.jsonl:12 and snapshot-5.jsonl line 12"), [("snapshot-5.jsonl", 12)])
        self.assertEqual(parse_citations("snapshot-5.jsonl records 2-4"), [("snapshot-5.jsonl", n) for n in (2, 3, 4)])
        self.assertEqual(parse_citations("no citation here"), [])

    def test_valid_only_when_the_record_states_the_fact(self):
        answer = "x\nCitation: snapshot-111.jsonl record "
        self.assertTrue(validate(self.dir, self.probe, answer + "1")["valid"])
        self.assertFalse(validate(self.dir, self.probe, answer + "2")["valid"])  # navigation copy
        self.assertFalse(validate(self.dir, self.probe, answer + "3")["valid"])  # only the recovery copy has it
        self.assertFalse(validate(self.dir, self.probe, answer + "9")["valid"])  # no such record
        missing = validate(self.dir, self.probe, "Citation: snapshot-999.jsonl record 1")
        self.assertEqual((missing["present"], missing["valid"]), (True, False))
        self.assertEqual(validate(self.dir, self.probe, "no cite"), {"present": False, "valid": False, "count": 0, "over_cap": False, "cited": []})
        self.assertTrue(validate(self.dir, {"evidence": r"lib\.rs:812:17"}, answer + "3")["valid"])

    def test_shotgun_citations_are_capped(self):
        three = "Citation: snapshot-111.jsonl records 9, 2, 1"
        self.assertTrue(validate(self.dir, self.probe, three)["valid"])
        four = "Citation: snapshot-111.jsonl records 9, 8, 2, 1"
        result = validate(self.dir, self.probe, four)
        self.assertFalse(result["valid"])
        self.assertTrue(result["over_cap"])
        self.assertEqual(result["count"], 4)
        self.assertFalse(validate(self.dir, self.probe, "snapshot-111.jsonl records 1-9")["valid"])
        self.assertIsNone(record_text(json.dumps({"evidence_kind": "recovery", "content": "x"})))


class SafetyTests(unittest.TestCase):
    def test_errors_are_reduced_to_a_class_and_status(self):
        raw = 'LLM request failed: Provider returned HTTP 429 Too Many Requests. Raw response: {"secret":"sk-abc"}'
        self.assertEqual(classify_error(raw), "provider_request:429")
        self.assertEqual(classify_error("Response Format Error: Failed to decode ... Raw response: {...}"), "provider_response")
        self.assertEqual(classify_error("Repeated unchanged tool results stopped the run."), "tool_loop")
        self.assertEqual(classify_error("boom at /Users/me/x"), "other")
        self.assertIsNone(classify_error(None))
        for cls in ("provider_request:429", "timeout", "other"):
            self.assertNotIn("sk-", cls)
        self.assertIsNone(harness.stored_answer({"answer": "", "error": "LLM request failed: 500"}))

    def test_runner_inputs_never_contain_the_answer_key(self):
        case = harness.load_cases("longform", ["longform-build-log"])[0]
        with tempfile.TemporaryDirectory() as scratch:
            inputs = harness.scratch_inputs(case, scratch)
            with open(os.path.join(inputs, "probes.json"), encoding="utf-8") as handle:
                text = handle.read()
            self.assertEqual(sorted(json.loads(text)[0]), ["id", "question"])
            self.assertNotIn("answer_text", text)

    def test_environment_is_isolated(self):
        with tempfile.TemporaryDirectory() as scratch:
            env = harness.isolated_env(scratch)
            self.assertTrue(env["HOME"].startswith(scratch) and env["TMPDIR"].startswith(scratch))
            self.assertNotIn("USER", env)
            self.assertEqual(set(env) - {"PATH", "OPENCODE_KEY", "SYSTEMROOT", "SYSTEMDRIVE", "SSL_CERT_FILE", "SSL_CERT_DIR"},
                             {"HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_DATA_HOME", "XDG_CONFIG_HOME", "TMPDIR", "TEMP", "TMP"})

    @unittest.skipUnless(sys.platform == "darwin" and shutil.which("sandbox-exec"), "macOS sandbox-exec only")
    def test_sandbox_blocks_reads_under_users(self):
        with tempfile.TemporaryDirectory() as scratch:
            runner = os.path.join(scratch, "runner")
            open(runner, "w").close()
            prefix = harness.sandbox_wrapper([runner], scratch, "auto")
            self.assertEqual(prefix[0], "sandbox-exec")
            ok = subprocess.run(prefix + ["/bin/cat", runner], capture_output=True)
            self.assertEqual(ok.returncode, 0)
            home_file = os.path.expanduser("~/.zshrc")
            if os.path.exists(home_file):
                blocked = subprocess.run(prefix + ["/bin/cat", home_file], capture_output=True)
                self.assertNotEqual(blocked.returncode, 0)
        self.assertEqual(harness.sandbox_wrapper(["/nonexistent"], "/tmp", "auto"), [])
        self.assertEqual(harness.sandbox_wrapper([sys.executable], "/tmp", "off"), [])


class ExclusionTests(unittest.TestCase):
    def test_probe_in_context_is_located(self):
        probe = {"evidence": anchored("brass astrolabe")}
        system = {"role": "System", "message_type": "Text", "content": "brass astrolabe"}
        checkpoint = {"role": "Assistant", "message_type": "Text", "content": "Earlier context checkpoint (x):\nthe brass astrolabe"}
        tail = {"role": "User", "message_type": "Text", "content": "now the brass astrolabe again"}
        other = {"role": "User", "message_type": "Text", "content": "nothing relevant"}
        self.assertIsNone(context_location([system, checkpoint.copy() | {"content": "Earlier context checkpoint:\nsummary"}, other], probe))
        self.assertEqual(context_location([system, checkpoint, other], probe), "checkpoint")
        self.assertEqual(context_location([system, checkpoint, tail], probe), "tail")

    def test_in_context_probes_leave_the_headline(self):
        def row(i, in_context, verdict="correct"):
            return {"case": "c", "id": str(i), "category": "detail", "domain": "code", "in_context": in_context,
                    "verdict": verdict, "answer": "a", "error": None, "cite_present": True, "cite_valid": verdict == "correct",
                    "tool_calls": 2, "denied": 0, "used_recall": False, "requests": 3, "input_tokens": 10, "output_tokens": 2,
                    "latency_ms": 1000}
        rows = [row(1, None), row(2, None, "wrong"), row(3, "tail"), row(4, "checkpoint")]
        run = {"chunks": [{"installed": True, "tokens_after": 5}], "recall_available": False}
        summary = trial_summary(rows, [run])
        self.assertEqual(summary["overall"]["probes"], 2)
        self.assertEqual(summary["overall"]["headline_correct_with_valid_citation"], 0.5)
        self.assertEqual((summary["excluded_in_context_tail"], summary["excluded_in_context_checkpoint"]), (1, 1))
        self.assertEqual(aggregate([summary, summary])["overall"]["probes"]["mean"], 2)


class Scripted(http.server.BaseHTTPRequestHandler):
    """Provider that checkpoints with a fixed summary and answers probes by reading the snapshots."""

    mode = "perfect"
    probes = {}

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        system = body["messages"][0]["content"]
        if "You summarize agent context" in system:
            message = {"role": "assistant", "content": SUMMARY}
            finish = "stop"
        elif body["messages"][-1]["role"] != "tool":
            name, arguments = "list_dir", {"directory_path": ".themis/context/eval-thread"}
            if self.mode == "snoop":  # try to read the fixture inputs next to the project by absolute path
                directory = json.loads(re.search(r"Recoverable evidence directory: (\"[^\"]+\")", system).group(1))
                workdir = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(directory))))
                name, arguments = "shell", {"command": "cat", "args": [os.path.join(workdir, "inputs", "probes.json")]}
            call = {"id": "call_1", "type": "function", "function": {"name": name, "arguments": json.dumps(arguments)}}
            message = {"role": "assistant", "content": None, "tool_calls": [call]}
            finish = "tool_calls"
        else:
            message = {"role": "assistant", "content": self.answer(body)}
            finish = "stop"
        payload = json.dumps({"id": "x", "object": "chat.completion", "usage": {"prompt_tokens": 100, "completion_tokens": 10, "total_tokens": 110},
                              "choices": [{"index": 0, "message": message, "finish_reason": finish}]}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def answer(self, body):
        question = next(m["content"] for m in reversed(body["messages"]) if m["role"] == "user").split("Question: ", 1)[1]
        probe = self.probes[question]
        if probe["category"] == "decoy" or self.mode in ("ignorant", "snoop"):
            return "UNKNOWN"
        directory = json.loads(re.search(r"Recoverable evidence directory: (\"[^\"]+\")", body["messages"][0]["content"]).group(1))
        cite = ""
        for name in sorted(os.listdir(directory)):
            if not name.endswith(".jsonl"):
                continue
            with open(os.path.join(directory, name), encoding="utf-8") as handle:
                for number, line in enumerate(handle, 1):
                    text = record_text(line)
                    if text and re.search(probe["evidence"], text, re.I | re.S) and not cite:
                        cite = f"\nCitation: .themis/context/eval-thread/{name} record {number if self.mode == 'perfect' else number + 1000}"
        return probe["answer_text"] + cite

    def log_message(self, *args):
        pass


@unittest.skipUnless(shutil.which("cargo"), "cargo is required to build the eval example")
class EndToEndTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        subprocess.run(["cargo", "build", "-q", "-p", "themis-core", "--features", "eval", "--example", "longform_eval"],
                       cwd=harness.ROOT, check=True)
        target = os.path.join(harness.ROOT, os.environ.get("CARGO_TARGET_DIR", "target"))
        cls.runner = os.path.join(target, "debug", "examples", "longform_eval" + harness.EXE)
        cls.case = harness.load_cases("longform", ["longform-build-log"])[0]
        Scripted.probes = {p["question"]: p for p in cls.case["probes"]}

    def run_mode(self, mode):
        Scripted.mode = mode
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Scripted)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        args = harness.argparse.Namespace(budget=24000, recent=20, max_calls=8, model="mock-model",
                                          base_url=f"http://127.0.0.1:{server.server_port}", timeout=600)
        report, error = harness.run_once([self.runner], self.case, args)
        self.assertIsNone(error)
        self.assertTrue(report["valid"], [c["error"][:120] for c in report["chunks"] if c["error"]][:1])
        self.assertEqual(len(report["rows"]), len(self.case["probes"]))
        return report

    def test_perfect_agent_scores_and_is_cited(self):
        report = self.run_mode("perfect")
        self.assertGreaterEqual(len(report["chunks"]), 3)
        original = [r for r in report["rows"] if r["category"] != "decoy" and r["in_context"] is None]
        self.assertGreaterEqual(len(original), 5)  # the mock summary keeps no facts
        for row in original:
            self.assertEqual(row["verdict"], "correct", row)
            self.assertTrue(row["cite_valid"], row)
            self.assertGreaterEqual(row["tool_calls"], 1)
        decoys = [r for r in report["rows"] if r["category"] == "decoy"]
        self.assertTrue(decoys and all(r["verdict"] == "correct_unknown" for r in decoys))
        summary = harness.build_summary([self.case], {(0, self.case["name"]): report}, 1)
        overall = summary["aggregate"]["overall"]
        self.assertEqual(overall["headline_correct_with_valid_citation"]["mean"], 1.0)
        self.assertEqual(summary["aggregate"]["decoy_hallucination_rate"]["mean"], 0.0)
        self.assertFalse(summary["aggregate"]["recall_available"])

    def test_wrong_citations_and_ignorance_do_not_pass(self):
        bad = self.run_mode("bad-citation")
        for row in bad["rows"]:
            if row["category"] != "decoy" and row["in_context"] is None:
                self.assertEqual(row["verdict"], "correct")
                self.assertTrue(row["cite_present"])
                self.assertFalse(row["cite_valid"], row)
        ignorant = self.run_mode("ignorant")
        for row in ignorant["rows"]:
            if row["category"] != "decoy":
                self.assertEqual(row["verdict"], "unknown")
        summary = harness.build_summary([self.case], {(0, self.case["name"]): ignorant}, 1)
        self.assertEqual(summary["aggregate"]["overall"]["headline_correct_with_valid_citation"]["mean"], 0.0)

    def test_reading_the_inputs_by_absolute_path_is_refused_and_recorded(self):
        report = self.run_mode("snoop")
        self.assertTrue(report["rows"])
        for row in report["rows"]:
            self.assertGreaterEqual(row["denied"], 1, row)
            self.assertTrue(any(p.endswith("probes.json") for p in row["refused_paths"]), row)
            self.assertEqual(row["executed_tool_calls"], 0)

    def test_scratch_workdirs_are_removed(self):
        before = {n for n in os.listdir(tempfile.gettempdir()) if n.startswith("longform-run-")}
        self.run_mode("perfect")
        after = {n for n in os.listdir(tempfile.gettempdir()) if n.startswith("longform-run-")}
        self.assertEqual(before, after)


if __name__ == "__main__":
    unittest.main()
