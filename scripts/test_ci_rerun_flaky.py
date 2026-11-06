import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import ci_rerun_flaky as rerun  # noqa: E402

CRATES = {"themis_core": "crates/core", "themis_desktop": "crates/desktop"}

# As GitHub's Windows runner prints it: colour codes, backslashes, CRLF.
WINDOWS_LOG = (
    "\x1b[1m\x1b[92m     Running\x1b[0m unittests src\\lib.rs (target\\debug\\deps\\themis_core-23d6.exe)\r\n"
    "test tools::tests::slow_one ... FAILED\r\n"
    "\x1b[1m\x1b[92m     Running\x1b[0m tests\\recall_e2e.rs (target\\debug\\deps\\recall_e2e-11aa.exe)\r\n"
    "test recalls_a_thing ... ok\r\n"
    "\x1b[1m\x1b[92m     Running\x1b[0m unittests src\\lib.rs (target\\debug\\deps\\themis_desktop-99ff.exe)\r\n"
    "\x1b[1m\x1b[92m     Running\x1b[0m tests\\recall_e2e.rs (target\\debug\\deps\\recall_e2e-22bb.exe)\r\n"
    "test a_download_nobody_allowed_is_cancelled ... FAILED\r\n"
    "failures:\r\n"
    "    a_download_nobody_allowed_is_cancelled\r\n"
    "test result: FAILED. 1 passed; 1 failed\r\n"
)


class ParseTests(unittest.TestCase):
    def test_failures_carry_their_binary_and_the_crate_that_built_it(self):
        failures = rerun.parse_failures(WINDOWS_LOG, CRATES)
        self.assertEqual(
            failures,
            [
                rerun.Failure("tools::tests::slow_one", "target/debug/deps/themis_core-23d6.exe", "crates/core"),
                # Both crates have a tests/recall_e2e.rs: the latest unit-test line decides.
                rerun.Failure("a_download_nobody_allowed_is_cancelled",
                              "target/debug/deps/recall_e2e-22bb.exe", "crates/desktop"),
            ],
        )

    def test_a_build_error_names_no_failed_test(self):
        log = "error[E0599]: no method named `snapshot`\nerror: could not compile `themis-core`\n"
        self.assertEqual(rerun.parse_failures(log, CRATES), [])

    def test_the_failures_list_at_the_end_is_not_counted_twice(self):
        log = (
            "     Running unittests src/lib.rs (target/debug/deps/themis_core-1)\n"
            "test a ... FAILED\nfailures:\n    a\n"
        )
        self.assertEqual(len(rerun.parse_failures(log, CRATES)), 1)

    def test_the_repositorys_crates_are_found(self):
        crates = rerun._crate_dirs(rerun.ROOT)
        self.assertEqual(crates.get("themis_core"), "crates/core")
        self.assertEqual(crates.get("themis_desktop"), "crates/desktop")


class MainTests(unittest.TestCase):
    def run_main(self, log, outcomes):
        import tempfile

        calls = []

        def fake_rerun(failure, root):
            calls.append(failure.name)
            return outcomes[failure.name]

        original = rerun.rerun
        rerun.rerun = fake_rerun
        try:
            with tempfile.NamedTemporaryFile("w", suffix=".log", delete=False) as handle:
                handle.write(log)
            code = rerun.main(["x", handle.name])
        finally:
            rerun.rerun = original
            Path(handle.name).unlink()
        return code, calls

    def test_all_passing_on_the_rerun_is_success_reported_as_flaky(self):
        code, calls = self.run_main(WINDOWS_LOG, {
            "tools::tests::slow_one": True, "a_download_nobody_allowed_is_cancelled": True})
        self.assertEqual((code, len(calls)), (0, 2))

    def test_one_failing_again_fails_the_job(self):
        code, _ = self.run_main(WINDOWS_LOG, {
            "tools::tests::slow_one": True, "a_download_nobody_allowed_is_cancelled": False})
        self.assertEqual(code, 1)

    def test_a_build_error_or_many_failures_are_not_rerun(self):
        code, calls = self.run_main("error: could not compile\n", {})
        self.assertEqual((code, calls), (1, []))
        many = "     Running unittests src/lib.rs (target/debug/deps/themis_core-1)\n" + "".join(
            f"test t{n} ... FAILED\n" for n in range(rerun.MAX_RERUNS + 1))
        code, calls = self.run_main(many, {})
        self.assertEqual((code, calls), (1, []))

    def test_the_report_names_flaky_and_repeated_failures(self):
        flaky = [rerun.Failure("a", "b", "crates/core")]
        again = [rerun.Failure("c", "d", "crates/desktop")]
        text = "\n".join(rerun.report(flaky, again))
        self.assertIn("FLAKY", text)
        self.assertIn("`a` (crates/core)", text)
        self.assertIn("Failed again", text)


if __name__ == "__main__":
    unittest.main()
