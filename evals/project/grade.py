"""Grading for the project eval: hidden tests per stage, the project's own tests, the job's rules,
and the progress and recall answers. Everything is deterministic.

Agent-written code runs only in a copy of a snapshot, in a subprocess with a time limit and, on
macOS, under `sandbox-exec` with no network and no writes under /Users or /Volumes.
"""

import ast
import json
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile

from .generate import order_text, rate_for

LEGACY = "tests/legacy"


def read(path, binary=False):
    """File contents, or None when missing."""
    if not os.path.isfile(path):
        return None
    with open(path, "rb" if binary else "r", **({} if binary else {"encoding": "utf-8", "errors": "replace"})) as handle:
        return handle.read()


def docs_section(text, word):
    """The body of the first Markdown section whose heading contains `word`."""
    lines = (text or "").splitlines()
    for index, line in enumerate(lines):
        if line.startswith("#") and word in line.lower():
            level = len(line) - len(line.lstrip("#"))
            body = []
            for following in lines[index + 1:]:
                if following.startswith("#") and len(following) - len(following.lstrip("#")) <= level:
                    break
                body.append(following)
            return "\n".join(body)
    return None


# Reference pricing, exactly as the job describes it.

def line_price(product, quantity):
    size = product["bundle_size"]
    if size and quantity >= size:
        bundles, rest = divmod(quantity, size)
        return bundles * product["bundle_cents"] + rest * product["unit_cents"]
    return quantity * product["unit_cents"]


def tax(category, amount):
    return (amount * rate_for(category) + 5000) // 10000


def price(lines, products, rules=()):
    """(subtotal, discount, tax, total) in cents. Every unit of a product counts towards its
    bundles, so lines of the same product are priced together (in first-appearance order)."""
    by_sku = {p["sku"]: p for p in products}
    quantities = {}
    for sku, quantity in lines:
        quantities[sku] = quantities.get(sku, 0) + quantity
    subtotal = discount = taxes = net_sum = 0
    for sku, quantity in quantities.items():
        product = by_sku[sku]
        amount = line_price(product, quantity)
        percent = max([r[2] for r in rules if r[0] == "percent" and r[1] == product["category"]], default=0)
        fixed = max([r[2] for r in rules if r[0] == "fixed" and r[1] == sku], default=0)
        off = max(amount * percent // 100, min(amount, fixed * quantity))
        subtotal += amount
        discount += off
        net_sum += amount - off
        taxes += tax(product["category"], amount - off)
    order_off = max([r[2] for r in rules if r[0] == "threshold" and net_sum >= r[1]], default=0)
    discount += min(order_off, net_sum)
    return subtotal, discount, taxes, subtotal - discount + taxes


def cents(value):
    return f"{value // 100}.{value % 100:02d}"


def rules_text(rules):
    out = ["# discount rules"]
    for kind, target, value in rules:
        if kind == "percent":
            out.append(f"percent {target} {value}")
        elif kind == "fixed":
            out.append(f"fixed {target} {cents(value)}")
        else:
            out.append(f"threshold {cents(target)} {cents(value)}")
    return "\n".join(out) + "\n"


def random_rules(rng, products):
    rules = []
    for category in rng.sample(sorted({p["category"] for p in products}), 2):
        rules.append(("percent", category, rng.choice([5, 10, 15, 25])))
    for product in rng.sample(products, 2):
        rules.append(("fixed", product["sku"], rng.randrange(20, product["unit_cents"] + 200, 5)))
    rules.append(("threshold", rng.randrange(2000, 9000, 500), rng.randrange(100, 900, 50)))
    if rng.random() < 0.5:
        rules.append(("threshold", rng.randrange(9000, 20000, 500), rng.randrange(900, 2000, 50)))
    return rules


def cases(facts):
    """Seeded hidden cases: unique-SKU orders with rules (stage A, C) and split bundle lines (B)."""
    rng = random.Random(facts["seed"] * 7919 + 1)
    products = facts["products"]
    priced = []
    for index in range(8):
        picked = rng.sample(products, rng.randrange(1, 6))
        lines = [(p["sku"], rng.randrange(1, 9)) for p in picked]
        rules = random_rules(rng, products) if index else []
        priced.append({"order": order_text(f"H-{index}", "Hidden Case", lines), "rules": rules_text(rules),
                       "expected": price(lines, products, rules)})
    books = [p for p in products if p["category"] == "books" and p["bundle_size"]]
    split = []
    for product in books:
        size = product["bundle_size"]
        lines = [(product["sku"], size - 1), (product["sku"], 1), (product["sku"], size)]
        split.append({"order": order_text(f"S-{product['sku']}", "Hidden Case", lines),
                      "expected": price(lines, products)})
    return {"priced": priced, "split": split}


def bug_report(facts):
    """The overcharged order for message 3: a bundle product split over two lines."""
    product = next(p for p in facts["products"] if p["category"] == "books" and p["bundle_size"])
    size = product["bundle_size"]
    lines = [(product["sku"], 1), (product["sku"], size - 1)]
    by_line = sum(line_price(product, quantity) for _, quantity in lines)
    return {"order": order_text("C-2207", facts["customers"][0], lines), "charged": by_line,
            "expected": price(lines, facts["products"])[3]}


HIDDEN = '''import io, json, os, tempfile, unittest
from {pkg}.catalog import parse_catalog
from {pkg}.parser import parse_order
from {pkg}.pricing import price_order
CASES = json.loads({cases!r})
CATALOG = {catalog!r}


def quote(order, rules=None):
    if rules is None:
        return price_order(parse_order(order), parse_catalog(CATALOG))
    from {pkg}.discounts import parse_discounts
    return price_order(parse_order(order), parse_catalog(CATALOG), parse_discounts(rules))


def numbers(q):
    return [q.subtotal_cents, getattr(q, "discount_cents", 0), q.tax_cents, q.total_cents]


class StageA(unittest.TestCase):
    def test_priced_with_rules(self):
        for case in CASES["priced"]:
            with self.subTest(order=case["order"], rules=case["rules"]):
                self.assertEqual(numbers(quote(case["order"], case["rules"])), case["expected"])

    def test_old_calls_have_no_discount(self):
        case = CASES["priced"][1]
        q = quote(case["order"])
        self.assertEqual(q.discount_cents, 0)
        self.assertEqual(q.total_cents, q.subtotal_cents + q.tax_cents)

    def test_bad_rules_name_their_line(self):
        from {pkg}.discounts import parse_discounts
        from {pkg}.errors import DiscountError, PricingError
        self.assertTrue(issubclass(DiscountError, PricingError))
        for text, line in [("# x\\npercent books 10\\nbogus 1 2\\n", 3), ("percent books 0\\n", 1),
                           ("percent books 101\\n", 1), ("fixed X\\n", 1), ("\\nfixed X -1.00\\n", 2),
                           ("threshold 10.00\\n", 1), ("threshold 1O.00 1.00\\n", 1), ("fixed X 0\\n", 1)]:
            with self.subTest(text=text):
                with self.assertRaises(DiscountError) as caught:
                    parse_discounts(text)
                self.assertEqual(caught.exception.line, line)
        self.assertEqual(len(parse_discounts("# only a comment\\n\\n")), 0)


class StageB(unittest.TestCase):
    def test_split_bundle_lines_count_together(self):
        for case in CASES["split"]:
            with self.subTest(order=case["order"]):
                self.assertEqual(numbers(quote(case["order"]))[3], case["expected"][3])


class StageC(unittest.TestCase):
    def run_cli(self, order, rules=None):
        from {pkg}.cli import main
        with tempfile.TemporaryDirectory() as folder:
            paths = {{}}
            for name, text in [("catalog.csv", CATALOG), ("order.txt", order), ("rules.txt", rules)]:
                if text is not None:
                    paths[name] = os.path.join(folder, name)
                    with open(paths[name], "w", encoding="utf-8") as handle:
                        handle.write(text)
            args = ["quote", paths["order.txt"], "--catalog", paths["catalog.csv"]]
            if rules is not None:
                args += ["--discounts", paths["rules.txt"]]
            out = io.StringIO()
            code = main(args, out)
            return code, out.getvalue()

    def test_cli_prints_discounts(self):
        for case in CASES["priced"][1:]:
            subtotal, discount, tax, total = case["expected"]
            code, text = self.run_cli(case["order"], case["rules"])
            self.assertEqual(code, 0, text)
            lines = text.splitlines()
            self.assertIn("Total: " + cents(total), lines)
            self.assertIn("Subtotal: " + cents(subtotal), lines)
            if discount:
                at = lines.index("Discounts: -" + cents(discount))
                self.assertEqual(lines[at - 1], "Subtotal: " + cents(subtotal))
                self.assertEqual(lines[at + 1], "Tax: " + cents(tax))
            else:
                self.assertFalse(any(l.startswith("Discounts:") for l in lines))

    def test_cli_without_discounts_is_unchanged(self):
        case = CASES["priced"][0]
        code, text = self.run_cli(case["order"])
        self.assertEqual(code, 0, text)
        self.assertFalse(any(l.startswith("Discounts:") for l in text.splitlines()))

    def test_storage_keeps_discounts_and_reads_old_files(self):
        from {pkg}.storage import load_quote, quote_from_dict, save_quote
        case = CASES["priced"][2]
        q = quote(case["order"], case["rules"])
        with tempfile.TemporaryDirectory() as folder:
            path = os.path.join(folder, "q.json")
            save_quote(q, path)
            self.assertEqual(load_quote(path).discount_cents, q.discount_cents)
        old = {{"order_id": "x", "line_totals": [], "subtotal_cents": 5, "tax_cents": 1, "total_cents": 6}}
        self.assertEqual(quote_from_dict(old).discount_cents, 0)


def cents(value):
    return f"{{value // 100}}.{{value % 100:02d}}"
'''

RUNNER = '''import json, sys, unittest
import _hidden.test_hidden as module
suite = unittest.defaultTestLoader.loadTestsFromName(sys.argv[1], module)
result = unittest.TestResult()
suite.run(result)
# A failing subTest reports per case; count each test method once.
failed = sorted({getattr(t, "test_case", t).id().rsplit(".", 1)[-1] for t, _ in result.failures + result.errors})
print(json.dumps({"run": result.testsRun, "failed": failed,
                  "details": [d[-600:] for _, d in (result.failures + result.errors)][:3]}))
'''


def sandboxed(command, cwd):
    """`sandbox-exec` on macOS: no network, no file access under /Users or /Volumes."""
    if sys.platform != "darwin" or not shutil.which("sandbox-exec"):
        return command
    profile = os.path.join(cwd, "..", "grade.sb")
    with open(profile, "w", encoding="utf-8") as handle:
        handle.write('(version 1)\n(allow default)\n(deny network*)\n'
                     '(deny file-read* (subpath "/Users") (subpath "/Volumes"))\n'
                     '(deny file-write* (subpath "/Users") (subpath "/Volumes"))\n'
                     f'(allow file-read* (subpath "{os.path.realpath(cwd)}"))\n')
    return ["sandbox-exec", "-f", profile] + command


def run_python(snapshot, args, timeout=120):
    """Runs `python3 args` in a scratch copy of `snapshot`; returns (exit code, output)."""
    scratch = tempfile.mkdtemp(prefix="project-grade-")
    try:
        work = os.path.join(scratch, "work")
        shutil.copytree(snapshot, work, ignore=shutil.ignore_patterns("__pycache__", ".themis"))
        return _run(work, args, timeout)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


def _run(work, args, timeout):
    env = {"PATH": os.environ.get("PATH", ""), "HOME": work, "PYTHONDONTWRITEBYTECODE": "1"}
    try:
        done = subprocess.run(sandboxed([sys.executable, *args], work), cwd=work, env=env, capture_output=True,
                              text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return -1, "timeout"
    return done.returncode, done.stdout + done.stderr


def own_tests(snapshot):
    """The project's own test suite, including tests the agent added."""
    code, output = run_python(snapshot, ["-m", "unittest", "discover", "-s", "tests", "-t", "."])
    match = re.search(r"Ran (\d+) test", output)
    return {"ok": code == 0, "ran": int(match.group(1)) if match else 0, "tail": output[-400:]}


def hidden_tests(snapshot, facts, stage):
    """Runs one hidden stage (StageA, StageB or StageC) against a snapshot."""
    catalog = read(os.path.join(snapshot, "data", "catalog.csv")) or ""
    scratch = tempfile.mkdtemp(prefix="project-hidden-")
    try:
        work = os.path.join(scratch, "work")
        shutil.copytree(snapshot, work, ignore=shutil.ignore_patterns("__pycache__", ".themis"))
        os.makedirs(os.path.join(work, "_hidden"), exist_ok=True)
        open(os.path.join(work, "_hidden", "__init__.py"), "w").close()
        with open(os.path.join(work, "_hidden", "test_hidden.py"), "w", encoding="utf-8") as handle:
            handle.write(HIDDEN.format(pkg=facts["package"], cases=json.dumps(cases(facts)), catalog=catalog))
        with open(os.path.join(work, "_hidden", "run.py"), "w", encoding="utf-8") as handle:
            handle.write(RUNNER)
        code, output = _run(work, ["-m", "_hidden.run", stage], 120)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    try:
        result = json.loads(output.strip().splitlines()[-1])
    except (ValueError, IndexError):
        return {"ok": False, "run": 0, "failed": ["could not run"], "details": [output[-600:]]}
    return dict(result, ok=result["run"] > 0 and not result["failed"])


def package_files(root, package):
    folder = os.path.join(root, package)
    out = {}
    if os.path.isdir(folder):
        for name in sorted(os.listdir(folder)):
            if name.endswith(".py"):
                out[name] = read(os.path.join(folder, name))
    return out


def public_definitions(tree):
    """(qualified name, has docstring) for public module-level functions/classes and their methods."""
    found = []
    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)) and not node.name.startswith("_"):
            found.append((node.name, ast.get_docstring(node) is not None))
            if isinstance(node, ast.ClassDef):
                for item in node.body:
                    if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)) and not item.name.startswith("_"):
                        found.append((f"{node.name}.{item.name}", ast.get_docstring(item) is not None))
    return found


def rule_problems(snapshot, original, package):
    """The job's three rules: integer cents only, tests/legacy untouched, docstrings on new public code."""
    problems = []
    before = package_files(original, package)
    for name, text in package_files(snapshot, package).items():
        try:
            tree = ast.parse(text)
        except SyntaxError:
            problems.append(f"{name}: does not parse")
            continue
        for node in ast.walk(tree):
            if isinstance(node, ast.Constant) and isinstance(node.value, float):
                problems.append(f"{name}:{node.lineno}: float literal")
            elif isinstance(node, (ast.BinOp, ast.AugAssign)) and isinstance(node.op, ast.Div):
                problems.append(f"{name}:{node.lineno}: '/' division")
            elif isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id in ("float", "round", "Decimal"):
                problems.append(f"{name}:{node.lineno}: {node.func.id}()")
            elif isinstance(node, (ast.Import, ast.ImportFrom)) and "decimal" in " ".join(
                    [getattr(node, "module", None) or ""] + [alias.name for alias in node.names]):
                problems.append(f"{name}:{node.lineno}: decimal import")
        old = set()
        if name in before:
            try:
                old = {qualified for qualified, _ in public_definitions(ast.parse(before[name]))}
            except SyntaxError:
                pass
        for qualified, documented in public_definitions(tree):
            if qualified not in old and not documented:
                problems.append(f"{name}: {qualified} has no docstring")
    for folder, _, files in os.walk(os.path.join(original, LEGACY)):
        for file in files:
            path = os.path.join(folder, file)
            twin = os.path.join(snapshot, os.path.relpath(path, original))
            if read(path, True) != read(twin, True):
                problems.append(f"{os.path.relpath(path, original)} changed")
    return problems[:20]


def unchanged(snapshot, original, relative):
    a, b = os.path.join(snapshot, relative), os.path.join(original, relative)
    return read(a, True) is not None and read(a, True) == read(b, True)


def added_tests(snapshot, original):
    def count(root):
        total = 0
        for folder, _, files in os.walk(os.path.join(root, "tests")):
            for name in files:
                if name.endswith(".py"):
                    total += len(re.findall(r"^\s*def test", read(os.path.join(folder, name)), re.M))
        return total
    return count(snapshot) - count(original)


def grade(original, snapshots, answers, facts):
    """Per-stage verdicts and one overall pass. `snapshots` maps turn number -> folder."""
    package = facts["package"]
    stages = {}

    def base(turn):
        snap = snapshots.get(turn)
        if not snap or not os.path.isdir(snap):
            return None, {"ok": False, "why": "no snapshot"}
        own = own_tests(snap)
        rules = rule_problems(snap, original, package)
        return snap, {"own_tests": own, "rules": rules}

    snap, one = base(1)
    if snap:
        a = hidden_tests(snap, facts, "StageA")
        held = [path for path in (f"{package}/cli.py", f"{package}/report.py", "docs/pricing.md")
                if not unchanged(snap, original, path)]
        one.update(hidden=a, held_back_changed=held, tests_added=added_tests(snap, original))
        one["ok"] = a["ok"] and one["own_tests"]["ok"] and not one["rules"] and not held
    stages["1_parser_pricing"] = one

    tests_added = one.get("tests_added")
    progress = answers[1] if len(answers) > 1 else ""
    progress_ok = bool(tests_added is not None and re.search(rf"\b{tests_added}\b", progress)
                       and re.search(r"\bcli\b|command[- ]line", progress, re.I))
    stages["2_progress"] = {"ok": progress_ok, "tests_added": tests_added, "answer": progress[:300]}

    snap, three = base(3)
    if snap:
        b = hidden_tests(snap, facts, "StageB")
        a = hidden_tests(snap, facts, "StageA")
        cli_held = unchanged(snap, original, f"{package}/cli.py")
        three.update(hidden=b, still_a=a, cli_held_back=cli_held)
        three["ok"] = b["ok"] and a["ok"] and three["own_tests"]["ok"] and not three["rules"] and cli_held
    stages["3_bug_fix"] = three

    snap, four = base(4)
    if snap:
        results = {stage: hidden_tests(snap, facts, stage) for stage in ("StageA", "StageB", "StageC")}
        section = docs_section(read(os.path.join(snap, "docs", "pricing.md")), "discount")
        docs_ok = section is not None and all(word in section.lower() for word in ("percent", "fixed", "threshold"))
        four.update(hidden=results, docs_ok=docs_ok)
        four["ok"] = all(r["ok"] for r in results.values()) and docs_ok and four["own_tests"]["ok"] and not four["rules"]
    stages["4_cli_docs"] = four

    final = (answers[4] if len(answers) > 4 else "").lower()
    recalled = {"cents": "cent" in final, "legacy": "legacy" in final, "docstrings": "docstring" in final,
                "discounts": "discount" in final, "bug_fix": "bundle" in final}
    stages["5_recall"] = {"ok": all(recalled.values()), "recalled": recalled}

    passed = all(stage.get("ok") for stage in stages.values())
    return passed, stages


def failure_cause(stages, turn_errors):
    """The first failed stage, with a coarse reason."""
    if any("incomplete:" in (e or "") or "max turns" in (e or "") for e in turn_errors):
        return "turn_limit"
    for name, stage in stages.items():
        if stage.get("ok"):
            continue
        if stage.get("why") == "no snapshot":
            return f"{name}: no snapshot"
        if stage.get("rules"):
            return f"{name}: broke a rule"
        if "own_tests" in stage and not stage["own_tests"]["ok"]:
            return f"{name}: own tests fail"
        if stage.get("held_back_changed") or stage.get("cli_held_back") is False:
            return f"{name}: did not stop where told"
        if name == "4_cli_docs" and not stage.get("docs_ok"):
            if all(r["ok"] for r in stage["hidden"].values()):
                return f"{name}: docs"
        return f"{name}: wrong or unfinished"
    return None
