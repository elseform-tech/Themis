import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

from quality_dashboard.dashboard import (
    ROOT,
    _test_names,
    parse_case_event,
    parse_lcov,
    parse_test_totals,
    rust_test_target,
)


class DashboardMetricsTests(unittest.TestCase):
    def test_rust_test_totals_sum_test_binaries(self):
        output = """
        test result: ok. 12 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out
        test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
        """
        self.assertEqual(parse_test_totals("rust", output), {"passed": 15, "failed": 1, "skipped": 2})

    def test_vitest_summary_reads_pass_fail_and_skipped(self):
        output = "Test Files  1 failed | 4 passed (5)\nTests  2 failed | 17 passed | 1 skipped (20)"
        self.assertEqual(parse_test_totals("frontend", output), {"passed": 17, "failed": 2, "skipped": 1})

    def test_python_unittest_summary_reads_failures_and_skips(self):
        output = "Ran 4 tests in 0.01s\nFAILED (failures=1, errors=1, skipped=1)"
        self.assertEqual(parse_test_totals("dashboard", output), {"passed": 1, "failed": 2, "skipped": 1})

    def test_lcov_requires_real_line_totals(self):
        self.assertEqual(parse_lcov("SF:a.rs\nDA:1,1\nLF:4\nLH:3\nend_of_record"), {"percent": 75.0, "hit": 3, "found": 4})
        self.assertIsNone(parse_lcov("SF:a.rs\nDA:1,1\nend_of_record"))

    def test_rust_and_frontend_coverage_remain_separate(self):
        from quality_dashboard import dashboard

        with TemporaryDirectory() as directory:
            root = Path(directory)
            for report_path, hit in (("coverage/lcov.info", 3), ("web/coverage/lcov.info", 1)):
                path = root / report_path
                path.parent.mkdir(parents=True)
                path.write_text(f"SF:source\nLF:4\nLH:{hit}\nend_of_record\n")
            with patch.object(dashboard, "ROOT", root):
                reports = dashboard.coverage_metrics()["reports"]
        self.assertEqual([(report["scope"], report["percent"]) for report in reports], [("Rust", 75.0), ("Frontend", 25.0)])

    def test_ast_metrics_use_visual_studio_index_and_ast_loc(self):
        from quality_dashboard.dashboard import _ast_complexity_row

        analysis = {
            "metrics": {
                "loc": {"sloc": 42},
                "nom": {"functions": 3},
                "cyclomatic": {"average": 4.5},
                "cognitive": {"average": 7.0},
                "mi": {"mi_visual_studio": 5.0},
            },
            "spaces": [
                {"kind": "function", "metrics": {"mi": {"mi_visual_studio": 70.0}}},
                {"kind": "class", "spaces": [
                    {"kind": "function", "metrics": {"mi": {"mi_visual_studio": 50.0}}},
                ]},
            ],
        }
        row = _ast_complexity_row("web/src/state/reducer.ts", analysis)
        self.assertEqual(row, {
            "path": "web/src/state/reducer.ts",
            "loc": 42,
            "functions": 3,
            "cyclomatic": 4.5,
            "cognitive": 7.0,
            "maintainability": 60.0,
            "maintainabilityTotal": 120.0,
            "maintainabilitySamples": 2,
        })

    def test_declaration_only_files_have_no_function_maintainability_score(self):
        from quality_dashboard.dashboard import _ast_complexity_row

        row = _ast_complexity_row("web/src/lib/types.ts", {"metrics": {"mi": {"mi_visual_studio": 0}}, "spaces": []})
        self.assertIsNone(row["maintainability"])
        self.assertEqual(row["maintainabilitySamples"], 0)

    def test_ast_output_parser_reads_multiple_records(self):
        from quality_dashboard.dashboard import _parse_ast_output

        output = '{\n  "name": "a.rs"\n}\n{\n  "name": "b.ts"\n}\n'
        self.assertEqual([item["name"] for item in _parse_ast_output(output)], ["a.rs", "b.ts"])

    def test_complexity_source_scope_includes_product_but_excludes_tooling_and_tests(self):
        from quality_dashboard.dashboard import _complexity_source_files

        paths = {path.relative_to(ROOT).as_posix() for path in _complexity_source_files()}
        self.assertTrue(any(path.startswith("crates/") for path in paths))
        self.assertTrue(any(path.startswith("web/src/") for path in paths))
        self.assertFalse(any(path.startswith(("quality_dashboard/", "scripts/")) for path in paths))

    def test_source_metrics_exclude_dashboard_tooling(self):
        from quality_dashboard.dashboard import source_metrics

        metrics = source_metrics()
        # Built-in skill verification scripts are product code, unlike dashboard tooling.
        self.assertEqual(metrics["byLanguage"].get("Python", {}).get("files"), len(list((ROOT / "crates").rglob("*.py"))))
        self.assertIn("Rust", metrics["byLanguage"])
        self.assertIn("TypeScript", metrics["byLanguage"])

    def test_test_inventory_separates_product_and_tooling_totals(self):
        from quality_dashboard.dashboard import test_inventory

        inventory = test_inventory()
        product_tests = [suite for suite in inventory["suites"] if suite["scope"] == "product"]
        tooling_tests = [suite for suite in inventory["suites"] if suite["scope"] == "tooling"]
        self.assertTrue(any(suite["path"] == "quality_dashboard/test_dashboard.py" for suite in tooling_tests))
        self.assertEqual(sum(suite["tests"] for suite in product_tests), inventory["total"])
        self.assertEqual(sum(suite["tests"] for suite in tooling_tests), inventory["toolingTotal"])
        self.assertEqual(inventory["discoveredTotal"], inventory["total"] + inventory["toolingTotal"])

    def test_inventory_contains_only_automated_suites(self):
        from quality_dashboard.dashboard import test_inventory

        inventory = test_inventory()
        self.assertNotIn("manualAcceptance", inventory)
        self.assertNotIn("manualE2e", inventory)
        self.assertEqual(inventory["overallTotal"], inventory["total"])
        self.assertEqual(inventory["overallDiscoveredTotal"], inventory["discoveredTotal"])
        self.assertTrue(any(suite["kind"] == "e2e" and "server_cli" in suite["path"] for suite in inventory["suites"]))

    def test_test_inventory_keeps_case_titles_and_runtime_events(self):
        names = _test_names(ROOT / "sample.test.ts", 'it("approves a safe read", () => {}); test.each(rows)("writes %s", () => {}); matcher.test("not a test case");')
        self.assertEqual(names, ["approves a safe read", "Parameterized test 2 (title resolved at runtime)"])
        self.assertEqual(parse_case_event("rust", "test crate::tools::allows_read ... ok", "crates/core/src/lib.rs", rust_modules={"tools": ["crates/core/src/tools.rs"]}), {"path": "crates/core/src/tools.rs", "name": "crate::tools::allows_read", "status": "passed"})
        self.assertEqual(parse_case_event("rust", "test version_matches_package_version ... ok", rust_test_names={"version_matches_package_version": ["crates/core/src/lib.rs"]}), {"path": "crates/core/src/lib.rs", "name": "version_matches_package_version", "status": "passed"})
        self.assertIsNone(parse_case_event("rust", "test shared_name ... ok", rust_test_names={"shared_name": ["a.rs", "b.rs"]})["path"])
        self.assertEqual(parse_case_event("frontend", "✓ src/screens/Settings.test.tsx > saves preferences 3ms", frontend_paths={"web/src/screens/Settings.test.tsx"}), {"path": "web/src/screens/Settings.test.tsx", "name": "src/screens/Settings.test.tsx > saves preferences", "status": "passed"})

    def test_rust_test_target_maps_unit_and_integration_binaries(self):
        sources = {
            "themis.rs": "crates/core/src/bin/themis.rs",
            "cli.rs": "crates/core/tests/cli.rs",
        }
        self.assertEqual(rust_test_target("Running unittests src/bin/themis.rs (target/debug/deps/themis)", sources), (True, "crates/core/src/bin/themis.rs"))
        self.assertEqual(rust_test_target("Running tests/cli.rs (target/debug/deps/cli)", sources), (True, "crates/core/tests/cli.rs"))
        self.assertEqual(rust_test_target("Running tests/unknown.rs (target/debug/deps/unknown)", sources), (True, None))
        self.assertEqual(rust_test_target("test cli::parses_arguments ... ok", sources), (False, None))


if __name__ == "__main__":
    unittest.main()
