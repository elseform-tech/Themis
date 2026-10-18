"""Long-horizon task: the spreadsheet engine, then three follow-up messages in the same chat that
each build on it (named ranges, text functions, a richer command line). One hidden test suite
runs at the end, covering every stage, so forgetting an earlier requirement fails too.
Run it with ordinary memory (`--memory normal`): the four turns grow past it on their own."""

from . import sheet

PROMPT = sheet.PROMPT

# Four turns of real work: give it three times the single task's limit.
TIMEOUT = 3 * 5400

FOLLOWUPS = [
    '''Now add named references to the engine.
- `sheet.name(name, target)` defines a name for a cell (like "B2") or a range (like "A1:A10").
  Names are letters, digits and underscores, start with a letter or underscore, and are
  case-insensitive. A name that is not valid, or that looks like a cell reference (like "AB12"),
  raises `SheetError` with code "#NAME?". A malformed target raises `SheetError` "#REF!".
  `sheet.name(name, "")` removes the name.
- `sheet.names()` returns a dict from each defined name (upper case) to its target (upper case).
- Formulas may use a name wherever its target could appear: `=TOTAL*2` for a cell name,
  `=SUM(PRICES)` for a range name. An undefined name in a formula is a "#NAME?" error.
- Redefining or removing a name changes every formula that uses it.
Keep everything that already works, add tests, and run the whole test suite.''',
    '''Now add text to formulas.
- String literals in double quotes: `="total: "`; a doubled quote inside is a quote
  (`="say ""hi"""` is `say "hi"`). A formula may result in text.
- `CONCAT(...)` joins its arguments as they display (an empty cell is ""); a range is joined row
  by row, left to right (A1, B1, A2, B2, ...). An error in any argument makes the result that
  error.
- `LEN(x)` is the number of characters of x as displayed; `UPPER(x)` and `LOWER(x)` change
  the case of x as displayed.
- `IFERROR(x, fallback)` is x, or fallback when x is an error (fallback is only evaluated then).
These work with names too. Keep everything that already works, add tests, and run the whole
test suite.''',
    '''Finally, extend the command line.
- `python -m sheet eval FILE.csv --raw` prints `to_csv(values=False)` (plain `eval` is unchanged).
- `python -m sheet set FILE.csv REF TEXT` sets one cell and writes the file back as raw CSV
  (`to_csv(values=False)`), creating the file when it does not exist. A malformed REF prints an
  error to stderr, exits with status 2 and leaves the file unchanged.
- `get` and the existing error handling stay as they are.
Then make sure every test passes, including the earlier ones, and tell me everything the
package now does.''',
]

EXTRA = r'''

class Names(unittest.TestCase):
    def test_names_cells_and_ranges(self):
        s = make(A1="2", A2="3", B1="10")
        s.name("prices", "a1:a2"); s.name("Rate", "B1")
        s.set("C1", "=SUM(PRICES)*rate"); self.assertEqual(show(s, "C1"), "50")
        self.assertEqual(s.names(), {"PRICES": "A1:A2", "RATE": "B1"})
    def test_names_follow_changes_and_removal(self):
        s = make(A1="2", B1="5", C1="=X+1")
        self.assertEqual(s.get("C1").code, "#NAME?")
        s.name("x", "A1"); self.assertEqual(show(s, "C1"), "3")
        s.name("x", "B1"); self.assertEqual(show(s, "C1"), "6")
        s.set("B1", "7"); self.assertEqual(show(s, "C1"), "8")
        s.name("x", ""); self.assertEqual(s.get("C1").code, "#NAME?"); self.assertEqual(s.names(), {})
    def test_names_bad_names_and_targets(self):
        s = Sheet()
        for bad in ("AB12", "1x", "a-b", ""):
            with self.subTest(bad=bad):
                with self.assertRaises(SheetError) as caught:
                    s.name(bad, "A1")
                self.assertEqual(caught.exception.code, "#NAME?")
        with self.assertRaises(SheetError) as caught:
            s.name("ok", "A0")
        self.assertEqual(caught.exception.code, "#REF!")

class TextFunctions(unittest.TestCase):
    def test_text_literals_and_concat(self):
        s = make(A1="2", B1="x", A2="=1/4", C1='="say ""hi"""')
        self.assertEqual(s.get("C1"), 'say "hi"')
        s.set("D1", '=CONCAT("n=", A1, " ", A1:B2)'); self.assertEqual(show(s, "D1"), "n=2 2x0.25")
        s.set("D2", "=CONCAT(A1, 1/0)"); self.assertEqual(s.get("D2").code, "#DIV/0!")
    def test_text_len_upper_lower(self):
        s = make(A1="Hello", A2="=1/3")
        s.set("B1", "=LEN(A1)"); self.assertEqual(show(s, "B1"), "5")
        s.set("B2", "=LEN(A2)"); self.assertEqual(show(s, "B2"), "12")
        s.set("B3", "=UPPER(A1)"); self.assertEqual(s.get("B3"), "HELLO")
        s.set("B4", '=LOWER("AbC")'); self.assertEqual(s.get("B4"), "abc")
    def test_text_iferror_and_names(self):
        s = make(A1="0", B1="word")
        s.set("C1", '=IFERROR(10/A1, "none")'); self.assertEqual(s.get("C1"), "none")
        s.set("A1", "4"); self.assertEqual(show(s, "C1"), "2.5")
        s.set("C2", '=IFERROR(1, 1/0)'); self.assertEqual(show(s, "C2"), "1")
        s.name("label", "B1"); s.set("C3", '=CONCAT(UPPER(label), "!")'); self.assertEqual(s.get("C3"), "WORD!")
    def test_text_in_arithmetic_is_still_an_error(self):
        s = make(A1='="7"', A2="=A1+1"); self.assertEqual(s.get("A2").code, "#VALUE!")

class CommandLineSet(unittest.TestCase):
    def run_cli(self, *args):
        return subprocess.run([sys.executable, "-m", "sheet", *args], capture_output=True, text=True, timeout=60)
    def test_cli2_raw_and_set(self):
        folder = tempfile.mkdtemp()
        path = os.path.join(folder, "s.csv")
        with open(path, "w") as handle:
            handle.write("2,3,=A1*B1\n")
        done = self.run_cli("eval", path, "--raw")
        self.assertEqual(done.returncode, 0, done.stderr); self.assertEqual(done.stdout, "2,3,=A1*B1\n")
        done = self.run_cli("set", path, "B1", "10")
        self.assertEqual(done.returncode, 0, done.stderr)
        with open(path) as handle:
            self.assertEqual(handle.read(), "2,10,=A1*B1\n")
        self.assertEqual(self.run_cli("get", path, "C1").stdout.strip(), "20")
        self.assertEqual(self.run_cli("eval", path).stdout, "2,10,20\n")
    def test_cli2_set_creates_and_rejects_bad_refs(self):
        folder = tempfile.mkdtemp()
        path = os.path.join(folder, "new.csv")
        done = self.run_cli("set", path, "B2", "=1+1")
        self.assertEqual(done.returncode, 0, done.stderr)
        with open(path) as handle:
            self.assertEqual(handle.read(), ",\n,=1+1\n")
        done = self.run_cli("set", path, "B0", "5")
        self.assertEqual(done.returncode, 2); self.assertTrue(done.stderr.strip())
        with open(path) as handle:
            self.assertEqual(handle.read(), ",\n,=1+1\n")
'''

TESTS = sheet.HIDDEN + EXTRA
# Hidden test names carry their stage: the follow-ups' tests are prefixed.
STAGES = (("names", "test_names_"), ("text", "test_text_"), ("command line", "test_cli2_"))


def setup(project):
    sheet.setup(project)


def stages(failed):
    """Which of the four stages pass: the base engine, then each follow-up."""
    passed = {"engine": not any(not name.startswith(tuple(p for _, p in STAGES)) for name in failed)}
    for stage, prefix in STAGES:
        passed[stage] = not any(name.startswith(prefix) for name in failed)
    return passed


def check(project, report):
    result = sheet.hidden(project, TESTS)
    passed_own, count = sheet.own_tests(project)
    by_stage = stages(result["failed"])
    found = []
    if result["failed"]:
        missing = [stage for stage, ok in by_stage.items() if not ok]
        found.append(f"stages failing: {', '.join(missing)} ({', '.join(result['failed'][:8])})")
    if count == 0:
        found.append("no own tests")
    elif not passed_own:
        found.append("own tests fail")
    return not found, "; ".join(found)


def measure(project, report):
    """Stages passed (of 4), for partial credit on a long task."""
    return {"stages_passed": sum(stages(sheet.hidden(project, TESTS)["failed"]).values())}
