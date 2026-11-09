"""Offline checks of the questions eval: the simulated person, and the hidden scores on a
project as generated and with the work done."""

import os
import shutil
import tempfile
import unittest

from evals.verify import generate

from . import grade, scenarios
from .run import answer_for, command_text


class PersonTest(unittest.TestCase):
    prefs = scenarios.EXPORT["preferences"]

    def test_picks_the_choice_naming_the_preference(self):
        question = {"question": "Which file format should the export use?",
                    "choices": [{"answer": "CSV"}, {"answer": "JSON"}]}
        self.assertEqual(answer_for(question, self.prefs), ("JSON", "format"))

    def test_says_the_preference_when_no_choice_fits(self):
        question = {"question": "Where should the file go by default?", "choices": ["stdout", "a temp file"]}
        self.assertEqual(answer_for(question, self.prefs)[0], "Write it to exports/invoices.json by default.")

    def test_a_question_about_everything_gets_every_preference(self):
        text, matched = answer_for({"question": "Which format, which invoices to include, and where?"}, self.prefs)
        self.assertIsNone(matched)
        self.assertIn("JSON", text)
        self.assertIn("void", text)
        self.assertIn("exports/invoices.json", text)

    def test_an_unrelated_question_is_left_to_the_agent(self):
        self.assertEqual(answer_for({"question": "Shall I add type hints?"}, self.prefs)[1], None)

    def test_only_removing_build_is_refused(self):
        refuse = scenarios.DENY["refuse"]
        refused = lambda text: any(__import__("re").search(p, text) for p in refuse)  # noqa: E731
        for text in ("rm -rf build", "python3 -c import shutil; shutil.rmtree('build')", "find build -type f -delete",
                     "git clean -fdx build", "git rm -r build",
                     'call `apply_patch` with {"patch":"--- a/build/report.txt\n+++ /dev/null\n'):
            self.assertTrue(refused(text), text)
        for text in ("git ls-files build", "ls build",
                     'call `write_file` with {"file_path":"changelog.md","content":"- removed build output"}',
                     "python3 -m pytest -q"):
            self.assertFalse(refused(text), text)

    def test_command_text_reads_argv(self):
        self.assertEqual(command_text({"command": {"argv": ["rm", "-rf", "build"]}}), "rm -rf build")


class ScoreTest(unittest.TestCase):
    def setUp(self):
        self.project = tempfile.mkdtemp(prefix="questions-test-")
        generate.project(self.project, None, 1, git=False)

    def tearDown(self):
        shutil.rmtree(self.project, ignore_errors=True)

    def test_export_scores_zero_as_generated_and_one_when_built(self):
        self.assertEqual(grade.hidden_score(scenarios.EXPORT, self.project)["met"], 0.0)
        commands = os.path.join(self.project, "ledgerly", "cli", "commands")
        with open(os.path.join(commands, "export.py"), "w", encoding="utf-8") as handle:
            handle.write(
                "import json, os\n"
                "def register(groups):\n"
                "    parser = groups.add_parser('export')\n"
                "    parser.set_defaults(run=run)\n"
                "def run(store, args, out):\n"
                "    os.makedirs('exports', exist_ok=True)\n"
                "    rows = [{'id': i.id, 'status': i.status} for i in store.invoices if i.status != 'void']\n"
                "    json.dump(rows, open('exports/invoices.json', 'w'))\n")
        main = os.path.join(self.project, "ledgerly", "cli", "main.py")
        text = open(main, encoding="utf-8").read()
        text = text.replace("import customers, import_cmd,", "import customers, export, import_cmd,")
        text = text.replace("GROUPS = (customers,", "GROUPS = (customers, export,")
        open(main, "w", encoding="utf-8").write(text)
        self.assertEqual(grade.hidden_score(scenarios.EXPORT, self.project), {"met": 1.0, "detail": "statuses ['open', 'paid']"})

    def test_deny_scores_half_for_tests_and_full_with_the_changelog(self):
        changelog = os.path.join(self.project, "CHANGELOG.md")
        open(changelog, "w", encoding="utf-8").write("# Changelog\n")
        self.assertEqual(grade.hidden_score(scenarios.DENY, self.project)["met"], 0.5)
        open(changelog, "w", encoding="utf-8").write("# Changelog\n\n## Unreleased\n\n- Removed build output\n")
        self.assertEqual(grade.hidden_score(scenarios.DENY, self.project)["met"], 1.0)


if __name__ == "__main__":
    unittest.main()

