import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

from quality_dashboard import ratchets
from quality_dashboard.ratchets import (
    LIMITS,
    Measurements,
    SourceFile,
    compare,
    duplicate_lines,
    file_measurements,
    function_values,
    out_of_line_test_modules,
    rust_test_items,
    tokenize,
)


def braces(tokens):
    return [token.text for token in tokens if token.kind == "punct" and token.text in "{}"]


def source(relative, text, rust=True, is_test=False):
    tokens = tokenize(text, rust)
    lines = set()
    if rust:
        for item in rust_test_items(tokens):
            lines.update(range(item.first_line, item.last_line + 1))
    return SourceFile(Path(relative), relative, text, tokens, lines, is_test)


def space(name, start, end, lines, cognitive, cyclomatic, children=(), kind="function"):
    return {
        "kind": kind,
        "name": name,
        "start_line": start,
        "end_line": end,
        "metrics": {
            "loc": {"sloc": lines},
            "cognitive": {"sum": cognitive},
            "cyclomatic": {"sum": cyclomatic},
        },
        "spaces": list(children),
    }


class LexerTests(unittest.TestCase):
    def test_rust_braces_inside_strings_comments_and_chars_are_not_code(self):
        text = r'''
        fn a<'a>(x: &'a str) {
            let s = "{"; let r = r#"}"#; let c = '{'; let e = '\''; let b = b'}';
            // }
            /* { /* nested } */ } */
            'outer: loop { break 'outer; }
        }
        '''
        self.assertEqual(braces(tokenize(text, rust=True)), ["{", "{", "}", "}"])

    def test_typescript_template_code_regex_and_jsx_apostrophes(self):
        text = (
            "const q = parts.map(part => `'${part.replace(/'/g, `'\"'\"'`)}'`).join(' ');\n"
            "const view = <p>{name}'s chat isn't {open ? `on ${x}` : \"off\"}</p>;\n"
            "if (a / b > c) { run(); }\n"
        )
        tokens = tokenize(text, rust=False)
        self.assertEqual(braces(tokens), ["{", "}", "{", "}", "{", "}"])
        self.assertTrue(all("\n" not in token.text for token in tokens))

    def test_token_lines_follow_multiline_strings(self):
        tokens = tokenize('let a = "x\ny";\nlet b = 1;', rust=True)
        self.assertEqual([token.line for token in tokens if token.text == "b"], [3])


class RustTestRegionTests(unittest.TestCase):
    def test_cfg_test_items_are_found_with_their_spans(self):
        text = "\n".join([
            "fn live() {}",
            "#[cfg(test)]",
            "mod tests {",
            '    const S: &str = "}";',
            "    #[test] fn t() {}",
            "}",
            "#[cfg(all(test, unix))]",
            "fn unix_only() { }",
            "#[cfg(not(test))]",
            "fn real() {}",
            "#[cfg(test)]",
            "mod scale_tests;",
        ])
        items = rust_test_items(tokenize(text, rust=True))
        self.assertEqual([(item.first_line, item.last_line) for item in items], [(2, 6), (7, 8), (11, 12)])
        self.assertEqual(items[-1].out_of_line_module, "scale_tests")

    def test_out_of_line_test_module_resolves_beside_its_parent(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "transcript").mkdir()
            (root / "transcript" / "scale_tests.rs").write_text("")
            (root / "lib.rs").write_text("")
            (root / "tests.rs").write_text("")
            items = rust_test_items(tokenize("#[cfg(test)] mod scale_tests;", rust=True))
            self.assertEqual(
                out_of_line_test_modules(root / "transcript.rs", items),
                {(root / "transcript" / "scale_tests.rs").resolve()},
            )
            items = rust_test_items(tokenize("#[cfg(test)] mod tests;", rust=True))
            self.assertEqual(out_of_line_test_modules(root / "lib.rs", items), {(root / "tests.rs").resolve()})

    def test_file_lines_count_tests_only_in_the_total(self):
        live = "\n".join(f"fn f{index}() {{}}" for index in range(5))
        tests = "\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}"
        measured = file_measurements([
            source("crates/a.rs", live + tests),
            source("crates/tests/e2e.rs", live, is_test=True),
        ])
        self.assertEqual(measured["crates/a.rs"], {"total_lines": 9, "code_lines": 5})
        self.assertEqual(measured["crates/tests/e2e.rs"], {"total_lines": 5})


class FunctionMetricTests(unittest.TestCase):
    def test_named_inner_functions_are_measured_apart_and_closures_fold_in(self):
        closure = space("<anonymous>", 3, 3, 1, 3, 2)
        helper = space("helper", 5, 14, 10, 4, 3)
        outer = space("Component", 1, 120, 120, 30, 25, [closure, helper])
        self.assertEqual(function_values(outer), {"lines": 110, "cognitive": 26, "cyclomatic": 22})
        rows = []
        ratchets._function_rows(space("unit", 1, 200, 200, 0, 0, [outer], kind="unit"), [], False, rows)
        self.assertEqual([path for path, _ in rows], [["Component"], ["Component", "helper"]])

    def test_repeated_names_get_a_counter(self):
        impl_a = space("impl Display for A", 1, 5, 5, 0, 1, [space("fmt", 2, 4, 3, 0, 1)], kind="impl")
        impl_b = space("impl Display for A", 6, 9, 4, 0, 1, [space("fmt", 7, 8, 2, 0, 1)], kind="impl")
        rows = []
        ratchets._function_rows(space("unit", 1, 9, 9, 0, 0, [impl_a, impl_b], kind="unit"), [], False, rows)
        self.assertEqual(
            [path for path, _ in rows],
            [["impl Display for A", "fmt"], ["impl Display for A#2", "fmt"]],
        )


BLOCK = "\n".join(f"    let value_{index} = compute(alpha, beta, {index});" for index in range(8))


class DuplicateTests(unittest.TestCase):
    def test_a_copied_block_counts_in_both_files(self):
        first = source("crates/a.rs", f"fn a() {{\n{BLOCK}\n}}")
        second = source("crates/b.rs", f"fn b(x: u8) {{\n    run();\n{BLOCK}\n}}")
        found = duplicate_lines([first, second])
        self.assertGreaterEqual(found["crates/a.rs"], 6)
        self.assertEqual(found["crates/a.rs"], found["crates/b.rs"])

    def test_a_dense_run_counts_when_the_whole_run_spans_enough_lines(self):
        # 80 identical tokens over 5 lines: its first 60 tokens fit in 4 lines.
        dense = "\n".join([
            "    c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());",
            "    c.stderr(std::process::Stdio::piped()).kill_on_drop(true);",
            "    for name in [\"PATH\", \"HOME\", \"SYSTEMROOT\", \"TMPDIR\"] {",
            "        if let Some(value) = std::env::var_os(name) {",
            "            c.env(name, value).env(name, value);",
        ])
        found = duplicate_lines([
            source("crates/a.rs", f"fn a() {{\n{dense}\n}}}}\n}}"),
            source("crates/b.rs", f"fn b(x: u8) {{\n    run(x);\n{dense}\n}}}}\n}}"),
        ])
        self.assertEqual(found, {"crates/a.rs": 7, "crates/b.rs": 7})

    def test_a_partial_match_is_counted_only_for_the_identical_part(self):
        tail_a = "\n".join(f"    finish_a({index});" for index in range(10))
        tail_b = "\n".join(f"    finish_b({index});" for index in range(10))
        found = duplicate_lines([
            source("crates/a.rs", f"fn a() {{\n{BLOCK}\n{tail_a}\n}}"),
            source("crates/b.rs", f"fn b() {{\n{BLOCK}\n{tail_b}\n}}"),
        ])
        self.assertEqual(found, {"crates/a.rs": 8, "crates/b.rs": 8})

    def test_tests_and_short_repeats_do_not_count(self):
        short = "    let x = compute(alpha, beta, 1);"
        found = duplicate_lines([
            source("crates/a.rs", f"fn a() {{\n{short}\n}}\nfn b() {{\n{short}\n}}"),
            source("crates/tests/x.rs", f"fn c() {{\n{BLOCK}\n}}", is_test=True),
            source("crates/c.rs", f"fn c() {{}}\n#[cfg(test)]\nmod tests {{\nfn d() {{\n{BLOCK}\n}}\n}}"),
            source("crates/d.rs", f"#[cfg(test)]\nmod tests {{\nfn e() {{\n{BLOCK}\n}}\n}}"),
        ])
        self.assertEqual(found, {})


def measurements(files=None, functions=None, duplicates=None):
    return Measurements(files or {}, functions or {}, duplicates or {})


class CompareTests(unittest.TestCase):
    def test_new_code_must_meet_the_limits(self):
        report = compare(
            measurements(files={"web/src/new.ts": {"total_lines": 1200, "code_lines": 1200}}), {},
        )
        self.assertEqual(report.grew, [f"file web/src/new.ts: code_lines 1200 > limit {LIMITS['file_code_lines']}"])
        self.assertEqual(report.baseline["files"], {})

    def test_recorded_code_may_shrink_but_not_grow(self):
        baseline = {"files": {"crates/big.rs": {"code_lines": 3000, "total_lines": 4000}}}
        grew = compare(measurements(files={"crates/big.rs": {"code_lines": 3001, "total_lines": 3500}}), baseline)
        self.assertEqual(grew.grew, ["file crates/big.rs: code_lines 3001 > recorded 3000"])
        self.assertEqual(grew.baseline["files"]["crates/big.rs"], {"code_lines": 3000, "total_lines": 3500})
        shrank = compare(measurements(files={"crates/big.rs": {"code_lines": 1500, "total_lines": 1800}}), baseline)
        self.assertEqual(shrank.grew, [])
        self.assertFalse(shrank.ok)  # the improvement has to be locked in
        self.assertEqual(shrank.baseline["files"], {"crates/big.rs": {"code_lines": 1500}})

    def test_a_recorded_function_cannot_add_a_new_problem(self):
        baseline = {"functions": {"crates/a.rs::run": {"lines": 150}}}
        report = compare(
            measurements(functions={"crates/a.rs::run": {"lines": 150, "cognitive": 40, "cyclomatic": 3}}), baseline,
        )
        self.assertEqual(report.grew, [f"function crates/a.rs::run: cognitive 40 > recorded {LIMITS['function_cognitive']}"])
        self.assertEqual(report.baseline["functions"], {"crates/a.rs::run": {"lines": 150}})

    def test_a_moved_function_keeps_its_allowance_but_a_renamed_new_one_does_not(self):
        baseline = {"functions": {"crates/a.rs::impl A::run": {"lines": 150}}}
        moved = compare(
            measurements(functions={"crates/a/run.rs::impl A::run": {"lines": 140, "cognitive": 1, "cyclomatic": 1}}),
            baseline,
        )
        self.assertEqual(moved.grew, [])
        self.assertEqual(moved.baseline["functions"], {"crates/a/run.rs::impl A::run": {"lines": 140}})
        renamed = compare(
            measurements(functions={"crates/a.rs::impl A::other": {"lines": 140, "cognitive": 1, "cyclomatic": 1}}),
            baseline,
        )
        self.assertEqual(len(renamed.grew), 1)
        self.assertEqual(renamed.baseline["functions"], {})

    def test_fixed_or_deleted_entries_leave_the_baseline(self):
        baseline = {"files": {"crates/gone.rs": {"code_lines": 2000}, "crates/fixed.rs": {"code_lines": 2000}}}
        report = compare(measurements(files={"crates/fixed.rs": {"code_lines": 900, "total_lines": 900}}), baseline)
        self.assertEqual(report.grew, [])
        self.assertEqual(len(report.improved), 2)
        self.assertEqual(report.baseline["files"], {})

    def test_repeated_code_total_only_goes_down(self):
        baseline = {"duplicate_lines_total": 100}
        grew = compare(measurements(duplicates={"crates/a.rs": 60, "crates/b.rs": 60}), baseline)
        self.assertEqual(len(grew.grew), 1)
        self.assertEqual(grew.baseline["duplicate_lines_total"], 100)
        pointed = compare(
            measurements(duplicates={"crates/a.rs": 50, "crates/new.rs": 12}),
            {"duplicate_lines_total": 50, "duplicate_lines_by_file": {"crates/a.rs": 50}},
        )
        self.assertIn("more than before in crates/new.rs (+12)", pointed.grew[0])
        shrank = compare(measurements(duplicates={"crates/a.rs": 40}), baseline)
        self.assertEqual((shrank.grew, shrank.baseline["duplicate_lines_total"]), ([], 40))

    def test_an_unchanged_codebase_passes(self):
        measured = measurements(
            files={"crates/big.rs": {"code_lines": 3000, "total_lines": 3000}, "crates/ok.rs": {"code_lines": 10, "total_lines": 10}},
            functions={"crates/big.rs::run": {"lines": 150, "cognitive": 3, "cyclomatic": 2}},
            duplicates={"crates/big.rs": 12},
        )
        baseline = ratchets.starting_baseline(measured)
        self.assertEqual(baseline["files"], {"crates/big.rs": {"code_lines": 3000, "total_lines": 3000}})
        report = compare(measured, baseline)
        self.assertTrue(report.ok, report)
        self.assertEqual(report.baseline, baseline)


if __name__ == "__main__":
    unittest.main()


class HealthScoreTests(unittest.TestCase):
    def test_each_part_counts_lines_and_repeated_code_is_scaled_to_five_percent(self):
        measured = measurements(
            files={
                "crates/big.rs": {"code_lines": 1500, "total_lines": 1500},
                "crates/ok.rs": {"code_lines": 500, "total_lines": 500},
                "crates/tests/e2e.rs": {"total_lines": 3000},
            },
            functions={
                "crates/ok.rs::long": {"lines": 150, "cognitive": 3, "cyclomatic": 2},
                "crates/ok.rs::fine": {"lines": 50, "cognitive": 3, "cyclomatic": 2},
            },
            duplicates={"crates/ok.rs": 20},
        )
        health = ratchets.health_score(measured)
        # Files: 1,500 of 2,000 code lines sit in a file over its limit (test files don't count).
        self.assertEqual(health["files"], 25.0)
        # Functions: 150 of 200 function lines sit in a function over a limit.
        self.assertEqual(health["functions"], 25.0)
        # Repeated: 20 of 2,000 lines is 1%, so 100 - 20 * 1 = 80.
        self.assertEqual(health["repeated"], 80.0)
        self.assertEqual(health["score"], 43.3)

    def test_five_percent_repeated_or_more_scores_zero_and_clean_code_scores_100(self):
        clean = measurements(
            files={"crates/a.rs": {"code_lines": 100, "total_lines": 100}},
            functions={"crates/a.rs::f": {"lines": 10, "cognitive": 1, "cyclomatic": 1}},
        )
        self.assertEqual(ratchets.health_score(clean)["score"], 100.0)
        copied = measurements(
            files={"crates/a.rs": {"code_lines": 100, "total_lines": 100}},
            functions={"crates/a.rs::f": {"lines": 10, "cognitive": 1, "cyclomatic": 1}},
            duplicates={"crates/a.rs": 9},
        )
        self.assertEqual(ratchets.health_score(copied)["repeated"], 0.0)

    def test_nothing_measured_scores_100_rather_than_dividing_by_zero(self):
        self.assertEqual(ratchets.health_score(measurements())["score"], 100.0)
