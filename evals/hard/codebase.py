"""Existing-codebase task: fix two reported bugs in a 1,400-line INI parser the agent did not
write, and add a feature, keeping its 340-test suite passing. It measures reading and
navigating code rather than building from scratch. The project is a copy of Python's
configparser as the package `iniconf` (see vendor/iniconf/NOTICE.md); grading runs the
original suite, so editing the project's copy of it does not help, plus hidden tests."""

import json
import os
import shutil
import subprocess
import sys
import tempfile

from evals.project.grade import sandboxed

from . import sheet

VENDOR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "vendor", "iniconf")

PROMPT = """This project is `iniconf`, an INI-file parser (the package in iniconf/, with its test
suite in tests/; run it with `python3 -m unittest discover -s tests`). Two bugs were reported:

1. Escaped percent signs come out doubled. With the default interpolation, a value written as
   `discount = 50%%` reads back as `50%%` instead of `50%`.
2. Values passed with `vars=` are not found when the key's case differs:
   `parser.get("db", "host", vars={"HOST": "override"})` returns the file's value instead of
   "override", although option names are case-insensitive everywhere else.

Fix both. Then add `getlist`:
- `parser.getlist(section, option, *, sep=",", raw=False, vars=None, fallback=...)`, like
  `getint`: the value split on `sep`, each item stripped of surrounding whitespace, empty items
  dropped (an empty value is `[]`). A missing option or section raises like `get`, unless a
  fallback is given, which is then returned as is.
- `parser[section].getlist(option, fallback=None, *, sep=",", raw=False, vars=None)` on section
  proxies, which like their `get` return the fallback (`None` by default) when it is missing.
- Interpolation applies as in `get` (none with `raw=True`).

Keep the whole existing test suite passing, add tests for what you changed, and tell me what
the causes of the two bugs were."""

EXTRA = r'''

class HiddenCodebase(unittest.TestCase):
    def parser(self, text, **kwargs):
        p = configparser.ConfigParser(**kwargs); p.read_string(text); return p
    def test_bug_percent_escapes_read_once(self):
        p = self.parser("[s]\na = 50%%\nb = %%(x)s and %%%%\nx = 1\n")
        self.assertEqual(p.get("s", "a"), "50%")
        self.assertEqual(p.get("s", "b"), "%(x)s and %%")
        self.assertEqual(p["s"]["a"], "50%")
        self.assertEqual(p.get("s", "a", raw=True), "50%%")
    def test_bug_vars_any_case(self):
        p = self.parser("[db]\nhost = a\nport = 1\n")
        self.assertEqual(p.get("db", "host", vars={"HOST": "x"}), "x")
        self.assertEqual(p["db"].get("Host", vars={"hOsT": "y"}), "y")
        self.assertEqual(p.getint("db", "port", vars={"PORT": "7"}), 7)
        self.assertEqual(p.get("db", "host"), "a")
    def test_getlist_splits_strips_and_drops_empty(self):
        p = self.parser("[s]\nitems = a, b ,, c ,\nempty =\nlines = one\n  two\n\n  three\n")
        self.assertEqual(p.getlist("s", "items"), ["a", "b", "c"])
        self.assertEqual(p.getlist("s", "empty"), [])
        self.assertEqual(p.getlist("s", "lines", sep="\n"), ["one", "two", "three"])
        self.assertEqual(p.getlist("s", "items", sep=";"), ["a, b ,, c ,"])
    def test_getlist_missing_and_fallbacks(self):
        p = self.parser("[s]\nitems = a\n")
        with self.assertRaises(configparser.NoOptionError):
            p.getlist("s", "missing")
        with self.assertRaises(configparser.NoSectionError):
            p.getlist("nope", "items")
        self.assertIsNone(p.getlist("s", "missing", fallback=None))
        self.assertEqual(p.getlist("nope", "items", fallback="as is"), "as is")
    def test_getlist_on_section_proxies(self):
        p = self.parser("[s]\nitems = a ; b\n")
        self.assertEqual(p["s"].getlist("items", sep=";"), ["a", "b"])
        self.assertIsNone(p["s"].getlist("missing"))
        self.assertEqual(p["s"].getlist("missing", ["z"]), ["z"])
    def test_getlist_interpolates_unless_raw(self):
        p = self.parser("[s]\nbase = x\nitems = %(base)s, y, 5%%\n")
        self.assertEqual(p.getlist("s", "items"), ["x", "y", "5%"])
        self.assertEqual(p.getlist("s", "items", raw=True), ["%(base)s", "y", "5%%"])
        self.assertEqual(p.getlist("s", "items", vars={"BASE": "v"}), ["v", "y", "5%"])
'''

# Tests in the original suite (`Ran N tests`, skipped ones included); the grader's self-test
# keeps this in step with vendor/iniconf/tests.
ORIGINAL_TESTS = 344
# Hidden test names carry their stage; every other test is the original suite.
STAGES = (("percent bug", "test_bug_percent_"), ("vars bug", "test_bug_vars_"), ("getlist", "test_getlist_"))


def _suite():
    with open(os.path.join(VENDOR, "tests", "test_iniconf.py"), encoding="utf-8") as handle:
        return handle.read()


def setup(project):
    for part in ("iniconf", "tests"):
        shutil.copytree(os.path.join(VENDOR, part), os.path.join(project, part), dirs_exist_ok=True)


def hidden(project):
    """{run, failed, details} for the original suite and the hidden tests against the agent's
    package: only `iniconf/` is taken from the project, beside the pristine tests."""
    scratch = tempfile.mkdtemp(prefix="hard-codebase-")
    try:
        work = os.path.join(scratch, "work")
        os.makedirs(os.path.join(work, "_hidden"))
        package = os.path.join(project, "iniconf")
        if os.path.isdir(package) and not os.path.islink(package):
            shutil.copytree(package, os.path.join(work, "iniconf"), symlinks=True,
                            ignore=shutil.ignore_patterns("__pycache__"))
        shutil.copytree(os.path.join(VENDOR, "tests", "configdata"), os.path.join(work, "_hidden", "configdata"))
        for name, text in (("__init__.py", ""), ("test_hidden.py", _suite() + EXTRA), ("run.py", sheet.RUNNER)):
            with open(os.path.join(work, "_hidden", name), "w", encoding="utf-8") as handle:
                handle.write(text)
        try:
            done = subprocess.run(sandboxed([sys.executable, "-B", "-E", "-s", "-m", "_hidden.run"], work), cwd=work,
                                  capture_output=True, text=True, timeout=300, check=False,
                                  env={"PATH": os.environ.get("PATH", "/usr/bin:/bin")})
        except subprocess.TimeoutExpired:
            return {"run": 0, "failed": ["timeout"], "details": []}
        try:
            return json.loads(done.stdout.strip().splitlines()[-1])
        except (ValueError, IndexError):
            return {"run": 0, "failed": ["import"], "details": [done.stderr[-500:]]}
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


def stages(result):
    """Which stages pass: the original suite, each bug, and getlist. A suite that did not run
    (a timeout, an import failure) passes none."""
    failed = result["failed"]
    if result.get("run", 0) == 0 or {"timeout", "import"} & set(failed):
        return {"suite": False, **{stage: False for stage, _ in STAGES}}
    prefixes = tuple(prefix for _, prefix in STAGES)
    passed = {"suite": not any(not name.startswith(prefixes) for name in failed)}
    for stage, prefix in STAGES:
        passed[stage] = not any(name.startswith(prefix) for name in failed)
    return passed


def check(project, report):
    result = hidden(project)
    by_stage = stages(result)
    report["_stages"] = by_stage
    found = []
    if not all(by_stage.values()):
        missing = [stage for stage, ok in by_stage.items() if not ok]
        found.append(f"stages failing: {', '.join(missing)} ({', '.join(result['failed'][:8])})")
    # The task asks for tests of the changes: the project's own suite must pass and have grown
    # past the original one.
    passed_own, count = sheet.own_tests(project)
    if not passed_own:
        found.append("own tests fail")
    elif count <= ORIGINAL_TESTS:
        found.append("no tests added")
    return not found, "; ".join(found)


def measure(project, report):
    """Stages passed (of 4), for partial credit."""
    by_stage = report.get("_stages") or stages(hidden(project))
    return {"stages_passed": sum(by_stage.values())}

