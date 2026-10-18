"""Long task: a spreadsheet engine in Python, graded by hidden tests run in a sandbox."""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

from evals.project.grade import sandboxed

PROMPT = """Build a small spreadsheet engine as a Python package `sheet` in this project (standard
library only, Python 3.10+), with tests in tests/. Work through it fully: it should all work when
you are done.

API (`from sheet import Sheet, SheetError`):
- `Sheet()`; `sheet.set(ref, text)` sets a cell's text ('' clears it); `sheet.text(ref)` returns
  the raw text ('' when empty); `sheet.get(ref)` returns the computed value; `sheet.display(ref)`
  returns it as shown.
- References are a column of one or two letters and a row from 1 to 999, case-insensitive: A1,
  b7, AA10. A malformed reference passed to `set`, `text`, `get` or `display` raises
  `SheetError` with `.code == "#REF!"`.
- Cell text: empty is empty; text that parses as a number (like 12, -3.5, .5) is that number; text
  starting with `=` is a formula; anything else is a text value.
- Computed values: `None` for an empty cell, a float for numbers, a str for text, or a
  `SheetError` instance (an Exception with a `code` attribute) for an error. `get` returns the
  error instance; it does not raise.

Formulas:
- Numbers, cell references, ranges like A1:B3 (only as function arguments), parentheses.
- Operators, loosest to tightest: comparisons `= <> < > <= >=` (1 for true, 0 for false), then
  `+ -`, then `* /`, then unary minus/plus, then `^` (right-associative, binds tighter than unary
  minus: -2^2 is -4 and 2^3^2 is 512).
- An empty cell counts as 0 in arithmetic; a text cell in arithmetic is a `#VALUE!` error.
- Functions (case-insensitive): SUM, AVG, MIN, MAX, COUNT over any mix of ranges and values
  (in ranges, only number cells count; text and empty cells are skipped; COUNT counts numbers);
  IF(condition, then, else) evaluates only the branch it takes; ABS(x); ROUND(x) and
  ROUND(x, digits), rounding halves away from zero.
- Errors: `#DIV/0!` (division by zero, AVG of no numbers), `#REF!` (bad reference),
  `#CYCLE!` (a cell that depends on itself, directly or not), `#NAME?` (unknown function),
  `#VALUE!` (text in arithmetic, a range outside a function, MIN/MAX of no numbers),
  `#ERROR!` (a formula that does not parse). An error in any input, including a cell in a range,
  makes the result that error.
- Values always reflect the current inputs: changing a cell changes everything that depends on it.

Display: empty is ''; errors show their code; text shows as is; a number with no fractional part
shows without a decimal point (3, -4, 0), others with up to 10 significant digits (2.5, 0.3333333333).

CSV:
- `Sheet.from_csv(text)`: the first row fills A1, B1, ...; the second row A2, B2, ...; empty
  fields stay empty.
- `sheet.to_csv(values=True)`: rows up to the last used row and columns up to the last used
  column, each cell as displayed (or as raw text with `values=False`), newline `\\n`, standard CSV
  quoting. An empty sheet is ''.

Command line: `python -m sheet eval FILE.csv` prints `to_csv()` of the file;
`python -m sheet get FILE.csv REF` prints one cell as displayed. A file that cannot be read prints
an error to stderr and exits with status 1.

Run your tests before you finish, then tell me what you built."""

HIDDEN = r'''
import os, subprocess, sys, tempfile, unittest
from sheet import Sheet, SheetError

def make(**cells):
    s = Sheet()
    for ref, text in cells.items():
        s.set(ref, text)
    return s

def show(s, ref):
    return s.display(ref)

class Basics(unittest.TestCase):
    def test_empty_and_text(self):
        s = make(A1="hello")
        self.assertIsNone(s.get("B2")); self.assertEqual(show(s, "B2"), "")
        self.assertEqual(s.get("A1"), "hello"); self.assertEqual(s.text("A1"), "hello")
    def test_numbers(self):
        s = make(A1="12", A2="-3.5", A3=".5")
        self.assertEqual(s.get("A1"), 12.0); self.assertEqual(s.get("A2"), -3.5); self.assertEqual(s.get("A3"), 0.5)
    def test_case_insensitive_refs(self):
        s = make(b7="4"); self.assertEqual(s.get("B7"), 4.0); self.assertEqual(s.text("b7"), "4")
    def test_two_letter_columns(self):
        s = make(AA10="2", A1="=AA10*3"); self.assertEqual(show(s, "A1"), "6")
    def test_bad_reference_raises(self):
        for ref in ("A0", "1A", "ABC1", "A1000", ""):
            with self.subTest(ref=ref):
                with self.assertRaises(SheetError) as caught:
                    Sheet().get(ref)
                self.assertEqual(caught.exception.code, "#REF!")
    def test_clear(self):
        s = make(A1="5"); s.set("A1", ""); self.assertIsNone(s.get("A1")); self.assertEqual(s.text("A1"), "")

class Arithmetic(unittest.TestCase):
    def test_precedence(self):
        s = make(A1="=2+3*4", A2="=(2+3)*4", A3="=10-4-3", A4="=12/4/3")
        self.assertEqual([show(s, r) for r in ("A1", "A2", "A3", "A4")], ["14", "20", "3", "1"])
    def test_power_and_unary(self):
        s = make(A1="=-2^2", A2="=2^3^2", A3="=(-2)^2", A4="=--3", A5="=+4")
        self.assertEqual([show(s, r) for r in ("A1", "A2", "A3", "A4", "A5")], ["-4", "512", "4", "3", "4"])
    def test_refs_and_empty_as_zero(self):
        s = make(A1="4", A2="=A1*2+B9")
        self.assertEqual(show(s, "A2"), "8")
    def test_comparisons(self):
        s = make(A1="=1<2", A2="=2<=1", A3="=3=3", A4="=3<>3", A5="=1+1>1", A6="=2>=2")
        self.assertEqual([show(s, r) for r in ("A1", "A2", "A3", "A4", "A5", "A6")], ["1", "0", "1", "0", "1", "1"])
    def test_display_of_fractions(self):
        s = make(A1="=1/3", A2="=5/2", A3="=0.1+0.2", A4="=1e3" if False else "=1000/8")
        self.assertEqual(show(s, "A1"), "0.3333333333"); self.assertEqual(show(s, "A2"), "2.5")
        self.assertEqual(show(s, "A3"), "0.3"); self.assertEqual(show(s, "A4"), "125")
    def test_whitespace_in_formulas(self):
        s = make(A1="= 1 + 2 * ( 3 - 1 )"); self.assertEqual(show(s, "A1"), "5")

class Functions(unittest.TestCase):
    def test_sum_avg_min_max_count(self):
        s = make(A1="1", A2="2", A3="x", A4="", B1="4")
        s.set("C1", "=SUM(A1:A4)"); s.set("C2", "=AVG(A1:A3)"); s.set("C3", "=MIN(A1:B1, 0.5)")
        s.set("C4", "=MAX(A1:A3, 3)"); s.set("C5", "=COUNT(A1:B4)")
        self.assertEqual([show(s, r) for r in ("C1", "C2", "C3", "C4", "C5")], ["3", "1.5", "0.5", "3", "3"])
    def test_function_names_any_case(self):
        s = make(A1="2", A2="=sum(A1, 3)"); self.assertEqual(show(s, "A2"), "5")
    def test_range_any_corner_order(self):
        s = make(A1="1", B2="2", C1="=SUM(B2:A1)"); self.assertEqual(show(s, "C1"), "3")
    def test_if_takes_one_branch(self):
        s = make(A1="5", B1="=IF(A1>3, 10, 1/0)", B2="=IF(A1>9, 1/0, 7)")
        self.assertEqual(show(s, "B1"), "10"); self.assertEqual(show(s, "B2"), "7")
    def test_abs_and_round(self):
        s = make(A1="=ABS(-2.5)", A2="=ROUND(2.5)", A3="=ROUND(-2.5)", A4="=ROUND(3.14159, 2)", A5="=ROUND(1234, -2)")
        self.assertEqual([show(s, r) for r in ("A1", "A2", "A3", "A4", "A5")], ["2.5", "3", "-3", "3.14", "1200"])
    def test_nested_calls(self):
        s = make(A1="2", A2="3", A3="=MAX(SUM(A1:A2), ABS(-10)) + IF(A1<A2, 1, 0)")
        self.assertEqual(show(s, "A3"), "11")

class Errors(unittest.TestCase):
    def code(self, s, ref):
        value = s.get(ref)
        self.assertIsInstance(value, SheetError, f"{ref} -> {value!r}")
        self.assertIsInstance(value, Exception)
        return value.code
    def test_div_zero(self):
        s = make(A1="=1/0", A2="=AVG(B1:B3)")
        self.assertEqual(self.code(s, "A1"), "#DIV/0!"); self.assertEqual(self.code(s, "A2"), "#DIV/0!")
    def test_cycle_direct_and_indirect(self):
        s = make(A1="=A1+1", B1="=B2", B2="=B3", B3="=B1*2")
        self.assertEqual(self.code(s, "A1"), "#CYCLE!")
        for ref in ("B1", "B2", "B3"):
            self.assertEqual(self.code(s, ref), "#CYCLE!")
    def test_cycle_through_range(self):
        s = make(A1="=SUM(A1:A3)"); self.assertEqual(self.code(s, "A1"), "#CYCLE!")
    def test_name(self):
        s = make(A1="=FOO(1)"); self.assertEqual(self.code(s, "A1"), "#NAME?")
    def test_value(self):
        s = make(A1="text", A2="=A1+1", A3="=A1:A2", A4="=MIN(B1:B2)")
        for ref in ("A2", "A3", "A4"):
            self.assertEqual(self.code(s, ref), "#VALUE!")
    def test_parse_errors(self):
        for text in ("=1+", "=(1", "=1 2", "=*3", "=SUM(1,"):
            with self.subTest(text=text):
                s = make(A1=text); self.assertEqual(self.code(s, "A1"), "#ERROR!")
    def test_bad_ref_inside_formula(self):
        s = make(A1="=ZZZ1+1"); self.assertIn(self.code(s, "A1"), ("#REF!", "#NAME?", "#ERROR!"))
    def test_errors_propagate(self):
        s = make(A1="=1/0", A2="=A1+1", A3="=SUM(A1:A2, 5)", B1="=FOO()", B2="=B1*0")
        self.assertEqual(self.code(s, "A2"), "#DIV/0!"); self.assertEqual(self.code(s, "A3"), "#DIV/0!")
        self.assertEqual(self.code(s, "B2"), "#NAME?")
    def test_error_display(self):
        s = make(A1="=1/0"); self.assertEqual(show(s, "A1"), "#DIV/0!")

class Recalculation(unittest.TestCase):
    def test_dependents_follow_changes(self):
        s = make(A1="1", A2="=A1*10", A3="=A2+A1")
        self.assertEqual(show(s, "A3"), "11")
        s.set("A1", "2"); self.assertEqual(show(s, "A3"), "22")
        s.set("A1", "x"); self.assertEqual(s.get("A3").code, "#VALUE!")
        s.set("A1", "3"); self.assertEqual(show(s, "A3"), "33")
    def test_breaking_a_cycle(self):
        s = make(A1="=B1", B1="=A1")
        self.assertEqual(s.get("A1").code, "#CYCLE!")
        s.set("B1", "7"); self.assertEqual(show(s, "A1"), "7")
    def test_range_sum_tracks_new_cells(self):
        s = make(C1="=SUM(A1:A5)"); s.set("A4", "9"); self.assertEqual(show(s, "C1"), "9")
    def test_long_chain(self):
        s = Sheet(); s.set("A1", "1")
        for row in range(2, 301):
            s.set(f"A{row}", f"=A{row - 1}+1")
        self.assertEqual(show(s, "A300"), "300")

class Csv(unittest.TestCase):
    def test_round_trip_values_and_raw(self):
        s = Sheet.from_csv("1,2,=A1+B1\nx,,=C1*2\n")
        self.assertEqual(show(s, "C2"), "6"); self.assertIsNone(s.get("B2"))
        self.assertEqual(s.to_csv(), "1,2,3\nx,,6\n")
        self.assertEqual(s.to_csv(values=False), "1,2,=A1+B1\nx,,=C1*2\n")
    def test_quoting(self):
        s = Sheet.from_csv('"a,b","=SUM(1,2)"\n')
        self.assertEqual(s.get("A1"), "a,b"); self.assertEqual(show(s, "B1"), "3")
        self.assertEqual(s.to_csv(), '"a,b",3\n')
    def test_extent_and_empty(self):
        self.assertEqual(Sheet().to_csv(), "")
        s = Sheet(); s.set("C2", "5"); self.assertEqual(s.to_csv(), ",,\n,,5\n")

class CommandLine(unittest.TestCase):
    def run_cli(self, *args):
        return subprocess.run([sys.executable, "-m", "sheet", *args], capture_output=True, text=True, timeout=60)
    def test_eval_and_get(self):
        with tempfile.NamedTemporaryFile("w", suffix=".csv", delete=False) as handle:
            handle.write("2,3,=A1^B1\n")
        try:
            done = self.run_cli("eval", handle.name)
            self.assertEqual(done.returncode, 0, done.stderr); self.assertEqual(done.stdout, "2,3,8\n")
            done = self.run_cli("get", handle.name, "c1")
            self.assertEqual(done.returncode, 0, done.stderr); self.assertEqual(done.stdout.strip(), "8")
        finally:
            os.unlink(handle.name)
    def test_missing_file(self):
        done = self.run_cli("eval", "/nonexistent/sheet.csv")
        self.assertEqual(done.returncode, 1); self.assertTrue(done.stderr.strip()); self.assertEqual(done.stdout, "")
'''

RUNNER = '''import json, sys, unittest
import _hidden.test_hidden as module
suite = unittest.defaultTestLoader.loadTestsFromModule(module)
result = unittest.TestResult()
suite.run(result)
failed = sorted({getattr(t, "test_case", t).id().rsplit(".", 1)[-1] for t, _ in result.failures + result.errors})
print(json.dumps({"run": result.testsRun, "failed": failed,
                  "details": [d[-500:] for _, d in (result.failures + result.errors)][:3]}))
'''


def setup(project):
    os.makedirs(os.path.join(project, "tests"), exist_ok=True)


def hidden(project, tests=None):
    """{run, failed, details} for the hidden tests (`tests`, default HIDDEN) against a copy of
    the project."""
    scratch = tempfile.mkdtemp(prefix="hard-sheet-")
    try:
        work = os.path.join(scratch, "work")
        shutil.copytree(project, work, ignore=shutil.ignore_patterns("__pycache__", ".themis", ".git"))
        os.makedirs(os.path.join(work, "_hidden"), exist_ok=True)
        for name, text in (("__init__.py", ""), ("test_hidden.py", tests or HIDDEN), ("run.py", RUNNER)):
            with open(os.path.join(work, "_hidden", name), "w", encoding="utf-8") as handle:
                handle.write(text)
        try:
            done = subprocess.run(sandboxed([sys.executable, "-m", "_hidden.run"], work), cwd=work,
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


def own_tests(project):
    """The agent's own tests: (passed, count)."""
    scratch = tempfile.mkdtemp(prefix="hard-sheet-own-")
    try:
        work = os.path.join(scratch, "work")
        shutil.copytree(project, work, ignore=shutil.ignore_patterns("__pycache__", ".themis", ".git"))
        try:
            # Without `-t`: a tests/ folder with or without __init__.py both count.
            done = subprocess.run(sandboxed([sys.executable, "-m", "unittest", "discover", "-s", "tests"], work),
                                  cwd=work, capture_output=True, text=True, timeout=300, check=False,
                                  env={"PATH": os.environ.get("PATH", "/usr/bin:/bin")})
        except subprocess.TimeoutExpired:
            return False, 0
        ran = re.search(r"Ran (\d+) test", done.stderr)
        return done.returncode == 0, int(ran.group(1)) if ran else 0
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


def check(project, report):
    result = hidden(project)
    passed_own, count = own_tests(project)
    found = []
    if result["failed"]:
        found.append(f"hidden tests failed: {', '.join(result['failed'][:8])}")
    # The spec asks for tests, not a number of them: they must exist and pass.
    if count == 0:
        found.append("no own tests")
    elif not passed_own:
        found.append("own tests fail")
    return not found, "; ".join(found)
