"""Offline tests for the long-horizon eval harness (no model, no network)."""

import tempfile
import unittest

from evals.tools.tasks import write

from .__main__ import notes_persistence, summarize
from .tasks import (
    CITIES,
    HEROES,
    MODULES,
    OWNER_RULE,
    TASKS,
    check_modules,
    check_stories,
    check_turns,
    measure_modules,
    measure_stories,
    module_functions,
    setup_modules,
    setup_stories,
)

A = [{"key": "goal", "text": "24 stories"}, {"key": "s01", "text": "Odile", "status": "done"}]
B = A + [{"key": "s02", "text": "Bram", "status": "in_progress"}]


class NotesPersistenceTest(unittest.TestCase):
    def test_verdicts(self):
        row = {"notes_log": [{"lines": A}, None, {"lines": B}], "checkpoints": [
            {"notes_calls_before": 0, "notes": None},
            {"notes_calls_before": 1, "notes": A},
            {"notes_calls_before": 3, "notes": A},
            {"notes_calls_before": 3, "notes": None},
            {"notes_calls_before": 3, "notes": [{"key": "other", "text": "x"}]},
            {"notes_calls_before": 3, "notes": B},
        ]}
        self.assertEqual(notes_persistence(row),
                         ["no_notes_yet", "kept", "stale", "dropped", "changed", "kept"])

    def test_summary_counts_only_checkpoints_after_notes(self):
        row = {"task": "stories-notes-asked", "arm": "notes-asked", "passed": True, "compactions": 2, "calls": [],
               "notes_log": [{"lines": A}], "final_notes": B,
               "checkpoints": [{"notes_calls_before": 0, "notes": None},
                               {"notes_calls_before": 1, "notes": A}],
               "extra": {"early_detail": True, "rereads": 2}, "why": ""}
        task = summarize([row])["tasks"]["stories-notes-asked"]
        self.assertEqual(task["notes_kept_rate"], 1.0)
        self.assertEqual(task["checkpoints_with_notes"], 1)
        self.assertEqual(task["open_at_end_rate"], 1.0)
        self.assertEqual(task["mean_rereads"], 2)


class TaskChecksTest(unittest.TestCase):
    def test_three_arms_per_job_and_only_the_asked_arm_asks(self):
        self.assertEqual(len(TASKS), 9)
        asked = [t for t in TASKS if t["arm"] == "notes-asked"]
        self.assertTrue(all("notes tool" in t["prompt"] for t in asked))
        self.assertFalse(any("notes tool" in t["prompt"] for t in TASKS if t["arm"] != "notes-asked"))
        self.assertTrue(all(t["offer_notes"] is False for t in TASKS if t["arm"] == "no-notes"))
        self.assertTrue(all(len(t["followups"]) == 3 for t in TASKS if t["job"] == "modules-turns"))

    def test_turns_check_needs_progress_and_the_rule_at_the_end(self):
        with tempfile.TemporaryDirectory() as project:
            setup_modules(project)
            counts = [len(module_functions(n)) for n in range(1, MODULES + 1)]
            for n in range(1, MODULES + 1):
                write(project, f"notes/mod-{n:02d}.md", OWNER_RULE + "\n" + "\n".join(module_functions(n))
                      + f"\nsummary: It is about the office in {CITIES[n - 1]}.\n")
            write(project, "notes/SUMMARY.md", "".join(f"mod-{n:02d}: {c}\n" for n, c in enumerate(counts, 1))
                  + f"total: {sum(counts)}\n")
            turn = lambda answer: {"answer": answer, "error": None}  # noqa: E731
            so_far = sum(counts[:8])
            good = {"turns": [turn("Done 1-8."), turn(f"8 of 16 done, 9-16 left, {so_far} functions so far."),
                              turn("Finished."), turn(f"First line '{OWNER_RULE}'; total {sum(counts)}.")],
                    "answer": "", "calls": []}
            self.assertEqual(check_turns(project, good), (True, ""))
            bad = dict(good, turns=good["turns"][:1] + [turn("Some are done.")] + good["turns"][2:])
            self.assertFalse(check_turns(project, bad)[0])

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
