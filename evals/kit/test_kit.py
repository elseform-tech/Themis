"""Offline tests for the shared eval kit (no app, no model, no network).

    python3 -m unittest evals.kit.test_kit
"""

import json
import os
import shutil
import sys
import tempfile
import unittest
from unittest import mock

from . import app, errors, git, spend
from .env import isolated_env

CATALOG = {"opencode-go": {"models": {
    "cheap": {"cost": {"input": 0.1, "output": 0.4, "cache_read": 0.01}, "limit": {"context": 128000}},
    "tiered": {"cost": {"input": 0.1, "output": 0.4, "context_over_200k": {"input": 0.5, "output": 1.0}},
               "limit": {"context": 1000000}},
    "dear": {"cost": {"input": 1.25, "output": 10}, "limit": {"context": 200000}},
    "free": {"cost": {"input": 0, "output": 0}, "limit": {"context": 64000}},
}}}


class EnvTests(unittest.TestCase):
    def test_only_path_key_and_scratch_folders_reach_the_app(self):
        folder = tempfile.mkdtemp()
        try:
            with mock.patch.dict(os.environ, {"OPENCODE_KEY": "k", "AWS_SECRET_ACCESS_KEY": "no",
                                              "GITHUB_TOKEN": "no", "PATH": "/bin"}):
                env = isolated_env(folder)
            self.assertEqual(env["OPENCODE_KEY"], "k")
            self.assertEqual(env["PATH"], "/bin")
            self.assertNotIn("AWS_SECRET_ACCESS_KEY", env)
            self.assertNotIn("GITHUB_TOKEN", env)
            for key in ("HOME", "TMPDIR", "XDG_CONFIG_HOME", "APPDATA"):
                self.assertTrue(env[key].startswith(folder), key)
            self.assertTrue(os.path.isdir(env["HOME"]))
            extra = set(env) - {"PATH", "OPENCODE_KEY", "HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA",
                                "XDG_DATA_HOME", "XDG_CONFIG_HOME", "TMPDIR", "TEMP", "TMP", "SYSTEMROOT",
                                "SYSTEMDRIVE", "SSL_CERT_FILE", "SSL_CERT_DIR"}
            self.assertEqual(extra, set())
        finally:
            shutil.rmtree(folder)


class ErrorTests(unittest.TestCase):
    def test_classes_keep_no_bodies(self):
        self.assertEqual(errors.classify_error("LLM request failed: HTTP 429 Too Many Requests {secret body}"),
                         "provider_request:429")
        self.assertEqual(errors.classify_error("request timed out"), "timeout")
        self.assertEqual(errors.classify_error("Stopped by you"), "stopped")
        self.assertEqual(errors.classify_error("weird"), "other")
        self.assertIsNone(errors.classify_error(""))
        self.assertTrue(errors.is_infrastructure("provider_response"))
        self.assertFalse(errors.is_infrastructure("stopped"))
        self.assertFalse(errors.is_infrastructure(None))


class GitTests(unittest.TestCase):
    def test_init_commit_and_dirty(self):
        folder = tempfile.mkdtemp()
        try:
            with open(os.path.join(folder, "a.txt"), "w", encoding="utf-8") as handle:
                handle.write("a")
            git.init(folder)
            self.assertEqual(git.commits(folder), 1)
            self.assertFalse(git.dirty(folder))
            with open(os.path.join(folder, "b.txt"), "w", encoding="utf-8") as handle:
                handle.write("b")
            self.assertTrue(git.dirty(folder))
            git.commit_all(folder, "more")
            self.assertEqual(git.commits(folder), 2)
        finally:
            shutil.rmtree(folder)


class SpendTests(unittest.TestCase):
    def test_check_model_refuses_unknown_and_dear_models(self):
        info, reason = spend.check_model(CATALOG, "cheap", 24000)
        self.assertEqual(info.price.input, 0.1)
        self.assertEqual(reason, "")
        self.assertIsNone(spend.check_model(CATALOG, "dear", 24000)[0])
        self.assertIn("not in models.dev", spend.check_model(CATALOG, "nope", 24000)[1])
        # A tiered model is cheap at a small budget but priced at its dearest tier on auto.
        self.assertIsNotNone(spend.check_model(CATALOG, "tiered", 24000)[0])
        self.assertIsNone(spend.check_model(CATALOG, "tiered", "auto")[0])
        self.assertIsNotNone(spend.check_model(CATALOG, "free", "auto")[0])

    def test_defaults(self):
        self.assertEqual((spend.MAX_INPUT_PRICE, spend.MAX_RUN_COST, spend.MAX_ROUND_COST), (0.20, 1.50, 5.00))

    def test_ledger_meter_takes_the_larger_count(self):
        meter = spend.LedgerMeter(spend.Price(0.1, 0.4))
        meter.update("run-1", {"requests": 2, "input_tokens": 1_000_000, "output_tokens": 0})
        self.assertAlmostEqual(meter.cost(), 0.1)
        # Goal checks and summaries are only in the ledger.
        meter.update_ledger(spend.ledger_counts({"windows": {"month": {
            "requests": 5, "input_tokens": 3_000_000, "cached_input_tokens": 0, "output_tokens": 1_000_000,
            "reasoning_tokens": 0, "unreported_requests": 0}}}))
        self.assertAlmostEqual(meter.cost(), 0.3 + 0.4)
        self.assertEqual(meter.usage()["requests"], 5)
        self.assertEqual(spend.ledger_counts(None)["requests"], None)

    def test_round_cap_holds_back_runs(self):
        price = spend.Price(0.1, 0.4)
        budget = spend.RoundSpend(price, max_round_cost=5.0, max_run_cost=1.5)
        self.assertEqual(budget.estimate("24000"), 1.5)
        for index in range(3):
            self.assertTrue(budget.can_start(budget.estimate("24000")))
            budget.finish(index, "24000", 1.5)
        self.assertFalse(budget.can_start(budget.estimate("24000")))
        cheaper = spend.RoundSpend(price, 5.0, 1.5)
        cheaper.finish(0, "24000", 0.4)
        self.assertEqual(cheaper.estimate("24000"), 0.4)
        running = spend.Meter(price)
        running.update(0, {"input_tokens": 20_000_000})
        self.assertTrue(cheaper.over_run_limit(running))
        free = spend.RoundSpend(spend.Price(0, 0), 0.0, 0.0)
        self.assertTrue(free.can_start(free.estimate("x")))


@unittest.skipIf(os.name == "nt", "uses a POSIX shell script as the CLI")
class CallTests(unittest.TestCase):
    def test_call_returns_json_or_the_error(self):
        folder = tempfile.mkdtemp()
        try:
            binary = os.path.join(folder, "cli")
            with open(binary, "w", encoding="utf-8") as handle:
                handle.write(f'#!/bin/sh\nexec "{sys.executable}" -c "{CLI}" "$@"\n')
            os.chmod(binary, 0o755)
            self.assertEqual(app.call(binary, folder, dict(os.environ), "echo", {"a": 1}), ({"a": 1}, None))
            value, error = app.call(binary, folder, dict(os.environ), "fail", {})
            self.assertIsNone(value)
            self.assertIn("boom", error)
            self.assertEqual(app.call(binary, folder, dict(os.environ), "empty"), (None, None))
            value, error = app.call(binary, folder, dict(os.environ), "slow", timeout=1)
            self.assertIn("no answer", error)
        finally:
            shutil.rmtree(folder)

    def test_keep_evidence_copies_the_run(self):
        folder, keep = tempfile.mkdtemp(), tempfile.mkdtemp()
        try:
            with open(os.path.join(folder, "trace.json"), "w", encoding="utf-8") as handle:
                json.dump({}, handle)
            app.keep_evidence(folder, keep, "run")
            copied = os.listdir(keep)
            self.assertEqual(len(copied), 1)
            self.assertTrue(os.path.isfile(os.path.join(keep, copied[0], "trace.json")))
        finally:
            shutil.rmtree(folder)
            shutil.rmtree(keep)


# argv: --data-dir DIR call METHOD JSON
CLI = ("import sys, time; m = sys.argv[4]; "
       "print(sys.argv[5]) if m == 'echo' else None; "
       "(sys.stderr.write('boom'), sys.exit(1)) if m == 'fail' else None; "
       "time.sleep(5) if m == 'slow' else None")


if __name__ == "__main__":
    unittest.main()



class KeychainGuard(unittest.TestCase):
    def test_a_found_entry_blocks_and_an_absent_one_does_not(self):
        from unittest import mock

        from evals.kit import env as env_kit
        with mock.patch.object(env_kit.sys, "platform", "darwin"), \
                mock.patch.object(env_kit.shutil, "which", return_value="/usr/bin/security"):
            for code, visible in ((0, True), (44, False)):
                with mock.patch.object(env_kit.subprocess, "run",
                                       return_value=mock.Mock(returncode=code)) as run:
                    self.assertEqual(env_kit.keychain_visible({"HOME": "/scratch"}), visible)
                    self.assertNotIn("-w", run.call_args.args[0])  # never asks for the secret
                    self.assertEqual(run.call_args.kwargs["env"], {"HOME": "/scratch"})

    def test_a_scratch_home_hides_the_login_keychain_here(self):
        import tempfile

        from evals.kit.env import isolated_env, keychain_visible
        with tempfile.TemporaryDirectory() as scratch:
            self.assertFalse(keychain_visible(isolated_env(scratch)))
