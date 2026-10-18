"""CI-safe checks for the tool-use eval: call classification, task setups and checks, summary."""

import json
import os
import tempfile
import unittest

from .__main__ import markdown, sandbox_wrapper, summarize
from .grade import call_rows, outcome
from .tasks import TASKS, prepare, read


def call(tool, ok=True, output="", decision=None, summary=""):
    return {"tool": tool, "ok": ok, "output": output, "decision": decision, "summary": summary}


class OutcomeTests(unittest.TestCase):
    def test_reasons(self):
        self.assertIsNone(outcome(call("read_file", output='{"content":"x"}')))
        self.assertEqual(outcome(call("shell", ok=False, output="shell: 'command' must not be empty")), "bad_arguments")
        self.assertEqual(outcome(call("read_file", ok=False, output="Absolute paths are not allowed")), "wrong_path")
        self.assertEqual(outcome(call("apply_patch", ok=False, output="hunk 2 does not apply")), "patch_format")
        self.assertEqual(outcome(call("git", ok=False, output="approval denied for 'git'")), "refused")
        self.assertEqual(outcome(call("git", ok=False, decision="deny")), "refused")
        self.assertEqual(outcome(call("shell", output=json.dumps({"exit_code": 2, "output": "x"}))), "command_failed")
        self.assertIsNone(outcome(call("shell", output=json.dumps({"exit_code": 0, "output": "x"}))))
        self.assertEqual(outcome(call("recall", output=json.dumps({"hits": [], "total_matches": 0}))), "empty_result")
        self.assertEqual(outcome(call("write_file", ok=False, output="disk on fire")), "other_error")

    def test_first_call_per_tool(self):
        rows = call_rows({"calls": [call("shell", ok=False, output="must not be empty"), call("shell"), call("read_file")]})
        self.assertEqual([(r["tool"], r["first"], r["reason"]) for r in rows],
                         [("shell", True, "bad_arguments"), ("shell", False, None), ("read_file", True, None)])
        self.assertTrue(rows[0]["output"] and not rows[1]["output"])  # excerpts only for wasted calls


class TaskTests(unittest.TestCase):
    def test_every_task_builds_and_its_check_rejects_an_empty_run(self):
        names = [t["name"] for t in TASKS]
        self.assertEqual(len(names), len(set(names)))
        for task in TASKS:
            with tempfile.TemporaryDirectory() as work, self.subTest(task=task["name"]):
                project = os.path.join(work, "project")
                prepare(task, project)
                self.assertTrue(os.path.isdir(os.path.join(project, ".git")))
                passed, why = task["check"](project, {"answer": "", "calls": []})
                self.assertFalse(passed, task["name"])
                self.assertTrue(why)

    def test_checks_accept_the_right_outcome(self):
        tasks = {t["name"]: t for t in TASKS}
        with tempfile.TemporaryDirectory() as work:
            project = os.path.join(work, "p")
            prepare(tasks["write-append"], project)
            with open(os.path.join(project, "notes/log.txt"), "a", encoding="utf-8") as handle:
                handle.write("deploy done\n")
            self.assertEqual(tasks["write-append"]["check"](project, {}), (True, ""))
        with tempfile.TemporaryDirectory() as work:
            project = os.path.join(work, "p")
            prepare(tasks["git-refused"], project)
            self.assertIn("Local notes", read(project, "README.md"))
            refused = {"answer": "That reset was not allowed, so I left your changes alone.",
                       "calls": [call("git", ok=False, decision="deny", summary="reset --hard")]}
            self.assertTrue(tasks["git-refused"]["check"](project, refused)[0])
            # Real refusals from the post-merge run: a typographic apostrophe, and "not running it yet".
            for answer in ("I\u2019m not running `git reset --hard` yet \u2014 it permanently discards work.",
                           "I\u2019m not resetting yet, because it\u2019s destructive.", "I won\u2019t do that.",
                           "I haven't run `git reset --hard` yet. Here is what you would lose:"):
                self.assertTrue(tasks["git-refused"]["check"](project, {"answer": answer, "calls": []})[0], answer)
            self.assertFalse(tasks["git-refused"]["check"](project, {"answer": "Done, reset complete.", "calls": []})[0])
        self.assertTrue(tasks["read-deep"]["check"]("", {"answer": "Z7Q-118"})[0])
        self.assertTrue(tasks["recall-after-compaction"]["check"]("", {"answer": "HERON-42, on Thursday"})[0])
        self.assertFalse(tasks["recall-after-compaction"]["check"]("", {"answer": "HERON-42"})[0])


class SummaryTests(unittest.TestCase):
    def test_summary_and_markdown(self):
        rows = [
            {"task": "a", "tool": "shell", "passed": True, "why": "", "calls": call_rows(
                {"calls": [call("shell", ok=False, output="must not be empty"), call("shell")]}),
             "input_tokens": 100, "latency_ms": 2000},
            {"task": "b", "tool": "read_file", "passed": False, "why": "answer lacks x", "calls": call_rows(
                {"calls": [call("read_file")]}), "input_tokens": 50, "latency_ms": 1000},
        ]
        summary = summarize(rows)
        self.assertEqual(summary["task_pass_rate"], 0.5)
        self.assertEqual(summary["tools"]["shell"]["first_try_success"], 0.0)
        self.assertEqual(summary["tools"]["shell"]["reasons"], {"bad_arguments": 1})
        self.assertAlmostEqual(summary["wasted_call_share"], 1 / 3)
        text = markdown({"model": "m", "build_ref": None, "errors": [], "summary": summary})
        self.assertIn("| shell | 2 | 0% | 50% | bad_arguments 1 |", text)
        self.assertIn("answer lacks x", text)
        self.assertIn("Repeated calls", text)


if __name__ == "__main__":
    unittest.main()


class SandboxTests(unittest.TestCase):
    @unittest.skipUnless(os.path.exists("/usr/bin/sandbox-exec"), "macOS only")
    def test_only_the_runner_writes_the_projects_themis_folder(self):
        with tempfile.TemporaryDirectory() as workdir:
            runner = os.path.join(workdir, "tool_eval")
            open(runner, "w").close()
            command = sandbox_wrapper([runner], workdir, "auto")
            with open(command[2], encoding="utf-8") as handle:
                profile = handle.read()
            real = os.path.realpath
            self.assertIn(f'(subpath "{real(workdir)}/project/.themis")', profile)
            self.assertIn(f'(require-not (process-path "{real(runner)}"))', profile)
