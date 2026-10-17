"""Offline tests for the project eval: the grader passes a correct solution and fails broken ones."""

import os
import shutil
import tempfile
import unittest

from .generate import generate
from .grade import bug_report, cases, failure_cause, grade, price, rule_problems
from .tasks import prompts

REFERENCE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "reference")


def edit(root, relative, old, new):
    path = os.path.join(root, relative)
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    assert old in text, (relative, old)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text.replace(old, new, 1))


def put(root, relative, template, package):
    with open(os.path.join(REFERENCE, template), encoding="utf-8") as handle:
        text = handle.read().replace("__pkg__", package)
    with open(os.path.join(root, relative), "w", encoding="utf-8") as handle:
        handle.write(text)


def stage_one(root, package):
    """The parser and pricing with discounts, and tests; CLI, report and docs untouched."""
    edit(root, f"{package}/errors.py", "class StorageError", '''class DiscountError(PricingError):
    """A discount rules file is malformed; `line` is the one-based line number."""

    def __init__(self, line, message):
        super().__init__(f"line {line}: {message}")
        self.line = line


class StorageError''')
    edit(root, f"{package}/models.py", "    total_cents: int\n", "    total_cents: int\n    discount_cents: int = 0\n")
    put(root, f"{package}/discounts.py", "discounts.py.tmpl", package)
    put(root, f"{package}/pricing.py", "pricing.py.tmpl", package)
    put(root, "tests/test_discounts.py", "test_discounts.py.tmpl", package)


def stage_four(root, package):
    """The CLI flag, the report line, storage and the docs section."""
    edit(root, f"{package}/report.py", '''    rows.append(f"Tax: ''', '''    if getattr(quote, "discount_cents", 0):
        rows.append(f"Discounts: -{format_cents(quote.discount_cents)}")
    rows.append(f"Tax: ''')
    edit(root, f"{package}/storage.py", '''        "total_cents": quote.total_cents,
''', '''        "total_cents": quote.total_cents,
        "discount_cents": quote.discount_cents,
''')
    edit(root, f"{package}/storage.py", '''            int(data["total_cents"]),
        )''', '''            int(data["total_cents"]),
            int(data.get("discount_cents", 0)),
        )''')
    edit(root, f"{package}/cli.py", '''    quote.add_argument("--json"''', '''    quote.add_argument("--discounts", help="discount rules file")
    quote.add_argument("--json"''')
    edit(root, f"{package}/cli.py", "        quote = price_order(order, catalog)\n", '''        rules = ()
        if args.discounts:
            with open(args.discounts, encoding="utf-8") as handle:
                rules = parse_discounts(handle.read())
        quote = price_order(order, catalog, rules)
''')
    edit(root, f"{package}/cli.py", "from __pkg__".replace("__pkg__", package) + ".errors",
         f"from {package}.discounts import parse_discounts\nfrom {package}.errors")
    with open(os.path.join(root, "docs", "pricing.md"), "a", encoding="utf-8") as handle:
        handle.write("\n## Discounts\n\nRules are percent (per category), fixed (per unit of a product) and "
                     "threshold (whole order).\n")


ANSWERS = ["Parser and pricing done.", "Done: parser, pricing. Left: CLI, report, docs. Tests added: 4.",
           "Fixed the split bundle lines.", "CLI and docs done.",
           "Rules: integer cents only, never change tests/legacy, docstrings on new public code. Changed: "
           "discounts, the bundle fix, CLI, report, storage and docs."]


class GraderTest(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.mkdtemp(prefix="project-eval-test-")
        self.original = os.path.join(self.folder, "original")
        self.facts = generate(self.original, 4)
        self.package = self.facts["package"]

    def tearDown(self):
        shutil.rmtree(self.folder, ignore_errors=True)

    def snapshots(self, mutate=None):
        """Turn snapshots of the reference solution; `mutate(turn, root)` breaks one on purpose."""
        snaps = {}
        current = os.path.join(self.folder, "work")
        shutil.copytree(self.original, current)
        for turn in range(1, 6):
            if turn == 1:
                stage_one(current, self.package)
            if turn == 4:
                stage_four(current, self.package)
            snap = os.path.join(self.folder, f"turn-{turn}")
            shutil.copytree(current, snap)
            if mutate:
                mutate(turn, snap)
            snaps[turn] = snap
        return snaps

    def test_reference_solution_passes_every_stage(self):
        passed, stages = grade(self.original, self.snapshots(), ANSWERS, self.facts)
        self.assertTrue(passed, {k: (v.get("ok"), v.get("hidden"), v.get("rules"), v.get("own_tests"))
                                 for k, v in stages.items()})
        self.assertEqual(stages["2_progress"]["tests_added"], 4)

    def test_untouched_project_fails_stage_one_but_keeps_the_rules(self):
        snaps = {turn: self.original for turn in range(1, 6)}
        passed, stages = grade(self.original, snaps, ANSWERS, self.facts)
        self.assertFalse(passed)
        self.assertFalse(stages["1_parser_pricing"]["ok"])
        self.assertEqual(stages["1_parser_pricing"]["rules"], [])
        self.assertTrue(stages["1_parser_pricing"]["own_tests"]["ok"])
        self.assertFalse(stages["3_bug_fix"]["hidden"]["ok"], "the planted bug is live in the template")

    def test_each_broken_variant_fails_where_expected(self):
        pkg = self.package
        variants = {
            "float": (1, lambda root: edit(root, f"{pkg}/discounts.py", "percent_of(amount, percent)",
                                           "int(amount * percent / 100.0)"), "1_parser_pricing: broke a rule"),
            "legacy": (3, lambda root: edit(root, "tests/legacy/test_legacy_quotes.py", "3440", "3441"),
                       "3_bug_fix: broke a rule"),
            "docstring": (1, lambda root: edit(root, f"{pkg}/discounts.py",
                                               '    """An amount in cents that must be above zero."""\n', ""),
                          "1_parser_pricing: broke a rule"),
            "early_cli": (1, lambda root: edit(root, f"{pkg}/cli.py", "price an order", "price one order"),
                          "1_parser_pricing: did not stop where told"),
            "no_docs": (4, lambda root: edit(root, "docs/pricing.md", "## Discounts", "## Offers"),
                        "4_cli_docs: docs"),
        }
        for name, (at, change, cause) in variants.items():
            with self.subTest(name):
                shutil.rmtree(os.path.join(self.folder, "work"), ignore_errors=True)
                for turn in range(1, 6):
                    shutil.rmtree(os.path.join(self.folder, f"turn-{turn}"), ignore_errors=True)
                snaps = self.snapshots(lambda turn, root: change(root) if turn == at else None)
                passed, stages = grade(self.original, snaps, ANSWERS, self.facts)
                self.assertFalse(passed, name)
                self.assertEqual(failure_cause(stages, []), cause, name)

    def test_hidden_failures_count_test_methods_not_cases(self):
        from .grade import hidden_tests
        snaps = self.snapshots()
        edit(snaps[1], f"{self.package}/discounts.py", "return min(best, net)", "return 0")
        result = hidden_tests(snaps[1], self.facts, "StageA")
        self.assertEqual(result["failed"], ["test_priced_with_rules"])
        self.assertLessEqual(len(result["failed"]), result["run"])

    def test_unfixed_bug_fails_stage_three(self):
        def unfix(turn, root):
            if turn == 3:
                edit(root, f"{self.package}/pricing.py", "quantities[line.sku] = quantities.get(line.sku, 0) + line.quantity",
                     "quantities[line.sku] = line.quantity")
        passed, stages = grade(self.original, self.snapshots(unfix), ANSWERS, self.facts)
        self.assertFalse(stages["3_bug_fix"]["hidden"]["ok"])
        self.assertTrue(stages["1_parser_pricing"]["ok"])

    def test_answers_are_checked(self):
        snaps = self.snapshots()
        wrong = list(ANSWERS)
        wrong[1] = "Some parts are done."
        wrong[4] = "We changed things."
        _, stages = grade(self.original, snaps, wrong, self.facts)
        self.assertFalse(stages["2_progress"]["ok"])
        self.assertFalse(stages["5_recall"]["ok"])
        self.assertEqual(failure_cause(stages, ["incomplete: handoff"]), "turn_limit")

    def test_bug_report_and_cases_follow_the_spec(self):
        report = bug_report(self.facts)
        self.assertGreater(report["charged"], report["expected"])
        message = prompts(self.facts)[2]
        self.assertIn(report["order"], message)
        self.assertIn(f"{report['charged'] // 100}.{report['charged'] % 100:02d}", message)
        hidden = cases(self.facts)
        self.assertEqual(len(hidden["priced"]), 8)
        self.assertTrue(hidden["split"])
        product = self.facts["products"][0]
        self.assertEqual(price([(product["sku"], 1)], self.facts["products"])[0], product["unit_cents"])
        self.assertEqual(rule_problems(self.original, self.original, self.package), [])


class ArmsTest(unittest.TestCase):
    def test_each_arm_changes_one_thing(self):
        from .__main__ import FILE_TOOLS, make_task
        base = make_task(1, "normal", "notes")
        self.assertTrue(base["offer_notes"])
        self.assertNotIn("nudges_off", base)
        self.assertFalse(make_task(1, "normal", "no-notes")["offer_notes"])
        for arm, nudge in [("no-reminder", "notes_reminder"), ("no-finish-checks", "finish_checks"),
                           ("no-loop-warning", "loop_warning"), ("no-repeat-receipt", "repeat_receipt")]:
            task = make_task(1, "normal", arm)
            self.assertEqual((task["nudges_off"], task["offer_notes"]), ([nudge], True))
        bash = make_task(1, "normal", "bash-only")
        self.assertEqual(bash["hide_tools"], FILE_TOOLS)
        self.assertNotIn("shell", bash["hide_tools"])
        reasoning = make_task(1, "normal", "no-reasoning")
        self.assertEqual((reasoning["replay_reasoning"], reasoning["offer_notes"]), (False, True))
        self.assertNotIn("replay_reasoning", base)


class TaskPlumbingTest(unittest.TestCase):
    def test_task_sets_prompts_grades_snapshots_and_measures(self):
        from evals.tools.tasks import prepare

        from .__main__ import make_task, markdown, summarize
        folder = tempfile.mkdtemp(prefix="project-task-test-")
        try:
            task = make_task(5, "normal", "notes")
            project = os.path.join(folder, "project")
            prepare(task, project)
            facts = generate(os.path.join(folder, "facts"), 5)
            self.assertIn(f"In this project ({facts['package']})", task["prompt"])
            self.assertEqual(len(task["followups"]), 4)
            self.assertTrue(task["snapshots"] and task["offer_notes"] and task["allow_all"])
            self.assertEqual(task["budget"], 200_000)
            current = os.path.join(folder, "current")
            shutil.copytree(project, current, ignore=shutil.ignore_patterns(".git", ".themis"))
            for turn in range(1, 6):
                if turn == 1:
                    stage_one(current, facts["package"])
                if turn == 4:
                    stage_four(current, facts["package"])
                shutil.copytree(current, f"{project}.turn-{turn}")
            report = {"turns": [{"answer": a, "error": None, "loop_warnings": 1, "finish_checks": 1} for a in ANSWERS]}
            passed, why = task["check"](project, report)
            self.assertTrue(passed, why)
            extra = task["measure"](project, report)
            self.assertTrue(all(extra["stage_ok"].values()))
            self.assertEqual((extra["final_hidden_pass"], extra["loop_warnings"]), (1.0, 5))
            row = {"task": task["name"], "passed": passed, "extra": extra, "compactions": 0, "calls": [],
                   "notes_log": [], "latency_ms": 60000, "input_tokens": 10}
            text = markdown({"model": "m", "errors": [], "summary": summarize([row])})
            self.assertIn("| project-normal-notes | 100% | 100% | 100% | 100% | 100% | 100% | 100% | 5.0 | 5.0 |", text)
        finally:
            shutil.rmtree(folder, ignore_errors=True)


if __name__ == "__main__":
    unittest.main()


class HiddenPassTest(unittest.TestCase):
    def test_the_share_never_goes_below_zero(self):
        from .__main__ import hidden_pass
        self.assertEqual(hidden_pass({}), 0.0)
        self.assertEqual(hidden_pass({"a": {"run": 4, "failed": ["x"]}}), 0.75)
        # An import error lists every planned test as failed though only one ran.
        self.assertEqual(hidden_pass({"a": {"run": 1, "failed": ["x", "y", "z"]}}), 0.0)
        self.assertEqual(hidden_pass({"a": {"run": 2, "failed": ["x", "x"]}}), 0.5)
