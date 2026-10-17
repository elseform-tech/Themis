"""Offline self-tests of the hard-task graders (no model involved)."""

import os
import shutil
import tempfile
import unittest

from . import sheet, threejs
from .__main__ import JOBS, make_task, notes_kept
from .browser import chrome

HERE = os.path.dirname(os.path.abspath(__file__))


def copy_reference(name):
    target = os.path.join(tempfile.mkdtemp(prefix="hard-ref-"), "project")
    shutil.copytree(os.path.join(HERE, "reference", name), target)
    return target


class SheetGraderTest(unittest.TestCase):
    def test_the_reference_passes_every_hidden_test(self):
        result = sheet.hidden(copy_reference("sheet"))
        self.assertGreaterEqual(result["run"], 30)
        self.assertEqual(result["failed"], [], result["details"])

    def test_a_broken_engine_fails_the_matching_tests(self):
        project = copy_reference("sheet")
        path = os.path.join(project, "sheet", "__init__.py")
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(text.replace('return ("bin", "^", base, self.unary())',
                                      'return ("bin", "^", base, self.atom())'))
        self.assertEqual(sheet.hidden(project)["failed"], ["test_power_and_unary"])

    def test_own_tests_count_with_or_without_a_tests_package(self):
        for package in (False, True):
            project = copy_reference("sheet")
            os.makedirs(os.path.join(project, "tests"), exist_ok=True)
            if package:
                open(os.path.join(project, "tests", "__init__.py"), "w").close()
            with open(os.path.join(project, "tests", "test_one.py"), "w", encoding="utf-8") as handle:
                handle.write("import unittest\nfrom sheet import Sheet\n\n"
                             "class T(unittest.TestCase):\n    def test_a(self):\n"
                             "        self.assertEqual(Sheet().display('A1'), '')\n")
            self.assertEqual(sheet.own_tests(project), (True, 1), package)

    def test_a_project_without_its_own_tests_fails(self):
        passed, why = sheet.check(copy_reference("sheet"), {})
        self.assertFalse(passed)
        self.assertIn("own tests", why)


@unittest.skipUnless(chrome(), "needs Chrome or Chromium")
class ThreejsGraderTest(unittest.TestCase):
    def project(self, mutate=None):
        project = os.path.join(tempfile.mkdtemp(prefix="hard-3d-"), "project")
        os.makedirs(project)
        threejs.setup(project)
        shutil.copytree(os.path.join(HERE, "reference", "threejs", "src"), os.path.join(project, "src"))
        shutil.copy(os.path.join(HERE, "reference", "threejs", "index.html"), project)
        if mutate:
            path = os.path.join(project, "src", "scene.js")
            with open(path, encoding="utf-8") as handle:
                text = handle.read()
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(mutate(text))
        return project

    def test_the_reference_scene_passes(self):
        self.assertEqual(threejs.check(self.project(), {}), (True, ""))

    def test_a_scene_without_fog_or_motion_fails(self):
        passed, why = threejs.check(self.project(lambda t: t.replace("scene.fog = ", "void ")
                                                 .replace("const angle = t * 0.1;", "const angle = 0;")), {})
        self.assertFalse(passed)
        self.assertIn("no fog", why)
        self.assertIn("camera does not move", why)

    def test_a_missing_module_is_reported(self):
        project = self.project()
        shutil.rmtree(os.path.join(project, "src"))
        passed, why = threejs.check(project, {})
        self.assertFalse(passed)
        self.assertIn("scene module", why)


class TaskTest(unittest.TestCase):
    def test_tasks_run_with_app_memory_or_squeezed_memory(self):
        for job in JOBS:
            self.assertEqual(make_task(job, "normal")["budget"], 200_000)
            self.assertEqual(make_task(job, "squeezed")["budget"], 32_000)
            self.assertTrue(make_task(job, "normal")["allow_all"])

    def test_the_notes_arm_withholds_notes_only(self):
        with_notes, without = make_task("sheet", "squeezed"), make_task("sheet", "squeezed", notes=False)
        self.assertTrue(with_notes["offer_notes"])
        self.assertFalse(without["offer_notes"])
        self.assertEqual(without["name"], "sheet-squeezed-no-notes")
        self.assertEqual({k: v for k, v in without.items() if k not in ("offer_notes", "notes", "name")},
                         {k: v for k, v in with_notes.items() if k not in ("offer_notes", "notes", "name")})

    def test_notes_kept_is_the_share_of_checkpoints_with_notes(self):
        self.assertIsNone(notes_kept({"checkpoints": []}))
        self.assertEqual(notes_kept({"checkpoints": [{"notes": [{"key": "goal"}]}, {"notes": None}]}), 0.5)
