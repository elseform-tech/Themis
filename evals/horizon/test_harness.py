"""Offline tests for the long-horizon eval harness (no model, no network)."""

import tempfile
import unittest

from evals.tools.tasks import write

from .__main__ import plan_persistence, summarize
from .tasks import (
    CITIES,
    HEROES,
    MODULES,
    OWNER_RULE,
    TASKS,
    check_modules,
    check_stories,
    measure_modules,
    measure_stories,
    module_functions,
    setup_modules,
    setup_stories,
)

A = [{"step": "read stories", "status": "in_progress"}]
B = [{"step": "read stories", "status": "done"}, {"step": "write report", "status": "in_progress"}]


class PlanPersistenceTest(unittest.TestCase):
    def test_verdicts(self):
        row = {"plans": [A, None, B], "checkpoints": [
            {"plan_calls_before": 0, "plan": None},
            {"plan_calls_before": 1, "plan": A},
            {"plan_calls_before": 3, "plan": A},
            {"plan_calls_before": 3, "plan": None},
            {"plan_calls_before": 3, "plan": [{"step": "other"}]},
            {"plan_calls_before": 3, "plan": B},
        ]}
        self.assertEqual(plan_persistence(row),
                         ["no_plan_yet", "kept", "stale", "dropped", "changed", "kept"])

    def test_summary_counts_only_checkpoints_after_a_plan(self):
        row = {"task": "stories-plan-asked", "arm": "plan-asked", "passed": True, "compactions": 2, "calls": [],
               "plans": [A], "checkpoints": [{"plan_calls_before": 0, "plan": None},
                                             {"plan_calls_before": 1, "plan": A}],
               "extra": {"early_detail": True, "rereads": 2}, "why": ""}
        task = summarize([row])["tasks"]["stories-plan-asked"]
        self.assertEqual(task["plan_kept_rate"], 1.0)
        self.assertEqual(task["checkpoints_with_plan"], 1)
        self.assertEqual(task["mean_rereads"], 2)


class TaskChecksTest(unittest.TestCase):
    def test_three_arms_per_job_and_only_the_asked_arm_asks(self):
        self.assertEqual(len(TASKS), 6)
        asked = [t for t in TASKS if t["arm"] == "plan-asked"]
        self.assertTrue(all("update_plan" in t["prompt"] for t in asked))
        self.assertFalse(any("update_plan" in t["prompt"] for t in TASKS if t["arm"] != "plan-asked"))
        self.assertTrue(all(t["offer_plan"] is False for t in TASKS if t["arm"] == "no-plan"))

    def test_stories_check_and_measures(self):
        with tempfile.TemporaryDirectory() as project:
            setup_stories(project)
            report = {"answer": "Done: Odile leads story-01 and there are 24 stories.",
                      "calls": [{"tool": "read_file", "summary": 'call `read_file` with {"path":"stories/story-01.txt"}'}] * 2}
            self.assertFalse(check_stories(project, report)[0])
            write(project, "report.txt", "".join(f"story-{n:02d}.txt: {h} walks.\n" for n, h in enumerate(HEROES, 1)))
            self.assertEqual(check_stories(project, report), (True, ""))
            self.assertEqual(measure_stories(project, report),
                             {"early_detail": True, "count_recalled": True, "rereads": 1})

    def test_modules_check_needs_the_rule_in_every_file(self):
        with tempfile.TemporaryDirectory() as project:
            setup_modules(project)
            counts = [len(module_functions(n)) for n in range(1, MODULES + 1)]
            for n in range(1, MODULES + 1):
                write(project, f"notes/mod-{n:02d}.md", OWNER_RULE + "\n" + "\n".join(module_functions(n))
                      + f"\nsummary: It is about the office in {CITIES[n - 1]}.\n")
            write(project, "notes/SUMMARY.md", "".join(f"mod-{n:02d}: {c}\n" for n, c in enumerate(counts, 1))
                  + f"total: {sum(counts)}\n")
            report = {"answer": f"Rule: first line '{OWNER_RULE}'. Total {sum(counts)}.", "calls": []}
            self.assertEqual(check_modules(project, report), (True, ""))
            self.assertEqual(measure_modules(project, report)["rule_kept_files"], MODULES)
            write(project, f"notes/mod-{MODULES:02d}.md", "\n".join(module_functions(MODULES)) + "\n")
            ok, why = check_modules(project, report)
            self.assertFalse(ok)
            self.assertIn("first line", why)
            self.assertEqual(measure_modules(project, report)["rule_kept_files"], MODULES - 1)


if __name__ == "__main__":
    unittest.main()
