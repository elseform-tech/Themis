"""Self-tests of the budget eval harness: offline and fast, never a live model.

    python3 -m unittest evals.budget.test_harness
"""

import hashlib
import http.server
import json
import os
import re
import shutil
import sys
import tempfile
import threading
import unittest
from unittest import mock

from evals.hard import sheet

from . import __main__ as harness
from . import execute, measures, spend
from .generate import SIZES, generate, hidden_tests, in_band, material_tokens, write_reference
from .grade import PROMPTS, fill, grade, messages, stages


def tree_digest(root):
    digest = hashlib.sha256()
    for folder, dirs, files in sorted(os.walk(root)):
        dirs.sort()
        for name in sorted(files):
            path = os.path.join(folder, name)
            digest.update(os.path.relpath(path, root).encode())
            with open(path, "rb") as handle:
                digest.update(handle.read())
    return digest.hexdigest()


class Project:
    """A generated project with the reference solution, optionally with one planted defect."""

    def __init__(self, size="small", seed=7):
        self.root = tempfile.mkdtemp(prefix="budget-test-")
        self.spec = generate(size, seed, self.root)
        self.tokens = material_tokens(self.root)
        write_reference(self.spec, self.root)

    def edit(self, relative, old, new):
        path = os.path.join(self.root, relative)
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
        assert old in text, (relative, old)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(text.replace(old, new, 1))

    def stages(self):
        result = sheet.hidden(self.root, hidden_tests(self.spec))
        return stages(result, [d.name for d in self.spec.desks])

    def close(self):
        shutil.rmtree(self.root, ignore_errors=True)


class GeneratorTests(unittest.TestCase):
    def test_same_seed_same_project_and_another_seed_another(self):
        roots = [tempfile.mkdtemp() for _ in range(3)]
        try:
            for root, seed in zip(roots, (3, 3, 4)):
                generate("small", seed, root)
            digests = [tree_digest(root) for root in roots]
            self.assertEqual(digests[0], digests[1])
            self.assertNotEqual(digests[0], digests[2])
        finally:
            for root in roots:
                shutil.rmtree(root, ignore_errors=True)

    def test_every_size_is_in_its_band(self):
        for size in SIZES:
            with self.subTest(size=size):
                root = tempfile.mkdtemp()
                try:
                    spec = generate(size, 11, root)
                    self.assertEqual(len(spec.desks), SIZES[size])
                    self.assertTrue(in_band(size, material_tokens(root)), material_tokens(root))
                finally:
                    shutil.rmtree(root, ignore_errors=True)

    def test_rules_live_only_on_their_own_page(self):
        root = tempfile.mkdtemp()
        try:
            spec = generate("small", 7, root)
            desk = spec.desks[0]
            pages = {}
            for d in spec.desks:
                with open(os.path.join(root, "docs", "desks", d.name + ".md"), encoding="utf-8") as handle:
                    pages[d.name] = handle.read()
            self.assertIn(f"{desk.bps} basis points", pages[desk.name])
            self.assertIn(f"`{desk.code}`", pages[desk.name])
            self.assertTrue(all(f"`{desk.code}`" not in page for name, page in pages.items() if name != desk.name))
            # The skeleton shipped to the agent states no rule.
            with open(os.path.join(root, "ledger", "desks", desk.name + ".py"), encoding="utf-8") as handle:
                self.assertNotIn(str(desk.floor), handle.read())
        finally:
            shutil.rmtree(root, ignore_errors=True)

    def test_the_change_and_the_tiers_are_both_exercised(self):
        for seed in range(1, 6):
            project = Project(seed=seed)
            try:
                tiers = {d.tier for d in project.spec.desks}
                self.assertIn(project.spec.changed_tier, tiers)
                self.assertTrue(tiers - {project.spec.changed_tier})
            finally:
                project.close()


class ReferenceTests(unittest.TestCase):
    def test_reference_passes_every_stage_at_every_size(self):
        for size in SIZES:
            with self.subTest(size=size):
                project = Project(size, seed=5)
                try:
                    graded = grade(project.root, project.spec)
                    self.assertTrue(graded["passed"], graded)
                    self.assertEqual(graded["stages_passed"], 3)
                    self.assertEqual(graded["desks_passed"], SIZES[size])
                finally:
                    project.close()

    def test_planted_defects_fail_exactly_their_stage(self):
        def wrong_rule(project):
            desk = project.spec.desks[0]
            project.edit(f"ledger/desks/{desk.name}.py", f"'bps': {desk.bps}", f"'bps': {desk.bps + 2}")

        def forgotten_rule(project):
            # Holds forget the stage-1 rejection rule: rejected entries are held or released too.
            project.edit("ledger/engine.py", "    accepted = [e for e in entries if rejection(rules, e) is None]\n"
                         "    released, held", "    accepted = list(entries)\n    released, held")

        def no_report(project):
            os.remove(os.path.join(project.root, "ledger", "__main__.py"))
        for plant, failing in ((wrong_rule, "stage1"), (forgotten_rule, "stage2"), (no_report, "stage3")):
            for seed in (2, 9):
                with self.subTest(defect=plant.__name__, seed=seed):
                    project = Project(seed=seed)
                    try:
                        plant(project)
                        by_stage, by_desk = project.stages()
                        self.assertEqual({s for s, ok in by_stage.items() if not ok}, {failing})
                        if failing != "stage3":
                            self.assertFalse(all(by_desk.values()))
                    finally:
                        project.close()

    def test_the_old_minimum_in_the_changed_tier_fails_stage_two_only(self):
        project = Project(seed=3)
        try:
            desk = next(d for d in project.spec.desks if d.tier == project.spec.changed_tier)
            project.edit(f"ledger/desks/{desk.name}.py", f"'min_fee': {project.spec.new_min}",
                         f"'min_fee': {desk.min_fee}")
            by_stage, by_desk = project.stages()
            self.assertEqual({s for s, ok in by_stage.items() if not ok}, {"stage2"})
            self.assertFalse(by_desk[desk.name])
        finally:
            project.close()

    def test_the_untouched_skeleton_fails_everything_and_has_no_own_tests(self):
        root = tempfile.mkdtemp()
        try:
            spec = generate("small", 4, root)
            graded = grade(root, spec)
            self.assertEqual(graded["stages_passed"], 0)
            self.assertEqual(graded["own_tests"]["count"], 0)
            self.assertIn("no own tests", graded["why"])
        finally:
            shutil.rmtree(root, ignore_errors=True)


class PromptTests(unittest.TestCase):
    def test_prompt_files_have_no_trailing_newline_and_fill_completely(self):
        for name in os.listdir(PROMPTS):
            with open(os.path.join(PROMPTS, name), encoding="utf-8") as handle:
                self.assertFalse(handle.read().endswith("\n"), name)
        root = tempfile.mkdtemp()
        try:
            spec = generate("small", 1, root)
            texts = messages(spec)
        finally:
            shutil.rmtree(root, ignore_errors=True)
        self.assertEqual(len(texts), 3)
        self.assertFalse(any("{{" in text for text in texts))
        self.assertIn(f"tier {spec.changed_tier} is now {spec.new_min} cents", texts[1])

    def test_fill_rejects_missing_and_unused_values(self):
        self.assertEqual(fill("a {{x}} b", x=1), "a 1 b")
        with self.assertRaises(ValueError):
            fill("a {{x}}")
        with self.assertRaises(ValueError):
            fill("a", x=1)


PRICE = spend.Price(input=0.10, output=0.40, cache_read=0.01)


class SpendTests(unittest.TestCase):
    def test_cost_counts_cached_input_at_its_price_and_covers_missing_usage(self):
        usage = {"input_tokens": 1_000_000, "cached_input_tokens": 600_000, "output_tokens": 100_000}
        self.assertAlmostEqual(spend.cost(PRICE, usage), 0.04 + 0.006 + 0.04)
        self.assertAlmostEqual(spend.cost(spend.Price(0.1, 0.4), usage), 0.1 + 0.04)
        missing = {"input_tokens": 0, "estimated_input_tokens": 2_000_000, "requests_missing_input_usage": 3}
        self.assertAlmostEqual(spend.cost(PRICE, missing), 0.2)
        self.assertEqual(spend.cost(spend.Price(0, 0), usage), 0)

    def test_meter_sums_the_latest_totals_of_each_turn(self):
        meter = spend.Meter(PRICE)
        meter.update(0, {"input_tokens": 100, "requests": 1})
        meter.update(0, {"input_tokens": 300, "requests": 2})
        meter.update(1, {"input_tokens": 50, "requests": 1})
        self.assertEqual(meter.usage()["input_tokens"], 350)
        self.assertEqual(meter.usage()["requests"], 3)

    def test_round_spend_holds_back_runs_that_would_pass_the_round_limit(self):
        round_ = spend.RoundSpend(PRICE, max_round_cost=1.0, max_run_cost=0.5)
        prior = round_.estimate("small", 30_000)
        self.assertGreater(prior, 0)
        self.assertLessEqual(round_.estimate("large", 10_000_000), 0.5)
        meter = spend.Meter(PRICE)
        round_.start("a", meter, 0.4)
        self.assertTrue(round_.can_start(0.4))
        meter.update(0, {"input_tokens": 5_000_000})  # live 0.5, above its estimate
        self.assertFalse(round_.can_start(0.6))
        self.assertTrue(round_.over_run_limit(spend.Meter(PRICE)) is False)
        meter.update(0, {"input_tokens": 6_000_000})
        self.assertTrue(round_.over_run_limit(meter))
        round_.finish("a", "small", meter.cost())
        self.assertAlmostEqual(round_.spent, 0.6)
        self.assertEqual(round_.estimate("small", 30_000), 0.5)  # highest seen, capped at the run limit
        self.assertFalse(round_.can_start(0.5))

    def test_free_models_are_never_held_back(self):
        free = spend.RoundSpend(spend.Price(0, 0), max_round_cost=0.0, max_run_cost=0.0)
        meter = spend.Meter(spend.Price(0, 0))
        meter.update(0, {"input_tokens": 10**9})
        self.assertTrue(free.can_start(free.estimate("large", 450_000)))
        self.assertFalse(free.over_run_limit(meter))

    def test_model_info_and_clamp(self):
        catalog = {"opencode-go": {"models": {"m": {"cost": {"input": 0.3, "output": 1.2}, "limit": {"context": 128000}}}}}
        info = spend.model_info(catalog, "m")
        self.assertEqual((info.price.input, info.price.cache_read, info.window), (0.3, None, 128000))
        self.assertIsNone(spend.model_info(catalog, "other"))
        self.assertEqual(spend.clamp(1_000_000, 128000), 128000)
        self.assertEqual(spend.clamp(64000, None), 64000)

    def test_context_tiers_price_a_run_by_its_budget(self):
        catalog = {"opencode-go": {"models": {"t": {"cost": {
            "input": 0.1, "output": 0.5, "cache_read": 0.01, "cache_write": 0.125,
            "tiers": [{"input": 0.2, "output": 0.75, "tier": {"type": "context", "size": 272000}}],
            "context_over_200k": {"input": 0.15, "output": 0.6}}}}}}
        info = spend.model_info(catalog, "t")
        self.assertEqual([size for size, _ in info.tiers], [200000, 272000])
        self.assertEqual(info.price_at(128000).input, 0.1)
        self.assertEqual(info.price_at(256000).input, 0.15)
        self.assertEqual(info.price_at(1000000).input, 0.2)
        # Uncached input is charged at the cache-write price when that is dearer.
        self.assertAlmostEqual(spend.cost(info.price, {"input_tokens": 1_000_000}), 0.125)
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "api.json")
            with open(path, "w", encoding="utf-8") as handle:
                json.dump(catalog, handle)
            refused = harness.main(["run", "--model", "t", "--catalog", path, "--via", "runner",
                                    "--budgets", "128000,400000", "--max-price", "0.18", "--dry-run"])
            allowed = harness.main(["run", "--model", "t", "--catalog", path, "--via", "runner",
                                    "--budgets", "128000,256000", "--max-price", "0.18", "--dry-run"])
        self.assertEqual((refused, allowed), (2, 0))

    def test_model_gone_reads_404_and_410_only(self):
        self.assertTrue(spend.model_gone("LLM request failed: HTTP 404 Not Found"))
        self.assertTrue(spend.model_gone("status: 410 gone"))
        self.assertTrue(spend.model_gone("error: model_not_found"))
        self.assertFalse(spend.model_gone("LLM request failed: HTTP 429 Too Many Requests"))
        self.assertFalse(spend.model_gone("read 14040 tokens"))

    def test_a_model_above_the_price_limit_is_refused_before_any_run(self):
        with tempfile.TemporaryDirectory() as tmp:
            catalog = os.path.join(tmp, "api.json")
            with open(catalog, "w", encoding="utf-8") as handle:
                json.dump({"opencode-go": {"models": {"pricey": {"cost": {"input": 0.5, "output": 2}}}}}, handle)
            code = harness.main(["run", "--model", "pricey", "--catalog", catalog, "--sizes", "small",
                                 "--budgets", "64000", "--trials", "1", "--out", os.path.join(tmp, "r.json")])
            self.assertEqual(code, 2)
            self.assertFalse(os.path.exists(os.path.join(tmp, "r.json")))


class ScheduleTests(unittest.TestCase):
    def test_arms_interleave_clamp_and_share_a_seed_per_trial(self):
        arms, dropped = harness.schedule(["small", "large"], [64000, 128000, 200000, 400000, 1000000], 3, 10,
                                         window=262144, via="runner")
        self.assertEqual(len(arms), 3 * 2 * 4)
        self.assertTrue(any("1000000" in line for line in dropped))
        self.assertEqual({a["budget"] for a in arms}, {64000, 128000, 200000, 262144})
        self.assertEqual({a["seed"] for a in arms if a["trial"] == 1}, {11})
        budgets = [a["budget"] for a in arms]
        # Not grouped by budget: the order changes budget far more often than a grouped one would.
        self.assertGreater(sum(a != b for a, b in zip(budgets, budgets[1:])), 8)
        self.assertEqual(arms, harness.schedule(["small", "large"], [64000, 128000, 200000, 400000, 1000000], 3,
                                                10, window=262144, via="runner")[0])

    def test_the_app_limit_is_read_from_the_configuration_schema(self):
        self.assertGreaterEqual(harness.app_budget_limit(), 256_000)


def cell(passes, cost, minutes):
    rows = [{"size": "large", "budget": None, "passed": p, "cost_usd": cost, "wall_s": minutes * 60,
             "failure": None if p else "wrong answer"} for p in passes]
    return rows


class DecisionTests(unittest.TestCase):
    def summary(self, arms):
        rows = []
        for budget, (passes, cost, minutes) in arms.items():
            rows += [dict(r, budget=budget) for r in cell(passes, cost, minutes)]
        return measures.summarize(rows)

    def test_smallest_budget_within_one_standard_error_of_the_best(self):
        summary = self.summary({64000: ([0, 0, 1], 0.1, 10), 128000: ([1, 1, 0], 0.2, 12),
                                200000: ([1, 1, 1], 0.3, 14), 400000: ([1, 1, 1], 0.5, 15)})
        chosen, why = measures.ceiling(summary)
        self.assertEqual(chosen, 200000, why)
        noisy = self.summary({64000: ([0, 1, 1], 0.1, 10), 200000: ([1, 0, 1, 1], 0.3, 14),
                              400000: ([1, 1, 1, 0], 0.5, 15)})
        self.assertEqual(measures.ceiling(noisy)[0], 64000)

    def test_a_tie_prefers_the_cheaper_and_faster_budget(self):
        summary = self.summary({128000: ([1, 1, 1], 0.9, 30), 200000: ([1, 1, 1], 0.4, 20)})
        self.assertEqual(measures.ceiling(summary)[0], 200000)
        summary = self.summary({128000: ([1, 1, 1], 0.9, 30), 200000: ([1, 1, 1], 0.4, 40)})
        self.assertEqual(measures.ceiling(summary)[0], 128000)

    def test_failure_kinds(self):
        good = {"grade": {"passed": True}, "finished": True}
        self.assertIsNone(measures.failure_kind(good))
        self.assertEqual(measures.failure_kind(dict(good, stopped="cost")), "stopped: cost")
        self.assertEqual(measures.failure_kind(dict(good, finished=False, error="max turns overall (5) reached")),
                         "turn limit")
        self.assertEqual(measures.failure_kind(dict(good, finished=False, error="Provider response was truncated")),
                         "cut-off")
        self.assertEqual(measures.failure_kind(dict(good, finished=False, error="LLM request failed: HTTP 502")),
                         "provider error")
        self.assertEqual(measures.failure_kind({"grade": {"passed": False}, "finished": True}), "wrong answer")

    def test_reread_share_after_checkpoints(self):
        read = lambda path: {"tool": "read_file", "summary": f'{{"file_path":"{path}"}}'}  # noqa: E731
        calls = [read("a.md"), read("b.md"), read("a.md"), read("c.md")]
        found = measures.reread_measures(calls, [{"calls_before": 2}])
        self.assertEqual((found["rereads_after_checkpoint"], found["reads_after_checkpoint"]), (1, 2))
        self.assertEqual(found["reread_share"], 0.5)
        self.assertIsNone(measures.reread_measures(None, None)["reread_share"])


# ---------------------------------------------------------------------------------------------
# End to end through the real tool-eval runner, against a scripted OpenAI-compatible server.

MARKERS = (("Implement `settle(entries)`", 1), ("Two changes, for every desk", 2), ("command-line report", 3))
USAGE = {"prompt_tokens": 1000, "completion_tokens": 100, "prompt_tokens_details": {"cached_tokens": 600},
         "completion_tokens_details": {"reasoning_tokens": 20}}


def _text(content):
    return content if isinstance(content, str) else json.dumps(content)


class ScriptedAgent(http.server.BaseHTTPRequestHandler):
    """Writes the reference solution in the first message, one file per request, then answers;
    later messages are answered at once. Every reply reports the same usage."""
    files = []
    requests = 0
    stream_seen = []

    def do_GET(self):
        self.reply({"object": "list", "data": [{"id": "mock-model", "object": "model"}]})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        type(self).requests += 1
        type(self).stream_seen.append(bool(body.get("stream")))
        messages = body.get("messages") or []
        stage, start = 0, 0
        for index, message in enumerate(messages):
            if message.get("role") == "user":
                found = next((n for marker, n in MARKERS if marker in _text(message.get("content"))), 0)
                if found:
                    stage, start = found, index
        done = sum(m.get("role") == "tool" for m in messages[start:])
        if stage == 1 and done < len(self.files):
            path, text = self.files[done]
            call = {"id": f"call-{done}", "type": "function",
                    "function": {"name": "write_file", "arguments": json.dumps({"file_path": path, "content": text})}}
            message, finish = {"role": "assistant", "content": "", "tool_calls": [call]}, "tool_calls"
        else:
            message, finish = {"role": "assistant", "content": f"Done with message {stage}."}, "stop"
        if body.get("stream"):
            delta = dict(message)
            delta.pop("role")
            if "tool_calls" in delta:
                delta["tool_calls"] = [dict(delta["tool_calls"][0], index=0)]
            chunks = [{"choices": [{"index": 0, "delta": delta, "finish_reason": finish}]},
                      {"choices": [], "usage": USAGE}]
            payload = "".join(f"data: {json.dumps(c)}\n\n" for c in chunks) + "data: [DONE]\n\n"
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(payload.encode())))
            self.end_headers()
            self.wfile.write(payload.encode())
            return
        self.reply({"id": "x", "object": "chat.completion", "usage": USAGE,
                    "choices": [{"index": 0, "message": message, "finish_reason": finish}]})

    def reply(self, value):
        payload = json.dumps(value).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *args):
        pass


def reference_files(size, seed):
    root = tempfile.mkdtemp()
    try:
        spec = generate(size, seed, root)
        before = {os.path.relpath(os.path.join(f, n), root) for f, _, ns in os.walk(root) for n in ns}
        write_reference(spec, root)
        files = []
        for folder, _, names in os.walk(root):
            for name in names:
                relative = os.path.relpath(os.path.join(folder, name), root)
                if relative.startswith(("ledger", "tests")) and relative.endswith(".py"):
                    with open(os.path.join(root, relative), encoding="utf-8") as handle:
                        text = handle.read()
                    if relative not in before or relative.startswith("ledger/desks/"):
                        files.append((relative, text))
        return sorted(files)
    finally:
        shutil.rmtree(root, ignore_errors=True)


@unittest.skipUnless(shutil.which("cargo"), "cargo is required to build the eval runner")
class RunnerEndToEndTests(unittest.TestCase):
    """Plumbing only: a scripted provider, never a quality claim about a model."""

    @classmethod
    def setUpClass(cls):
        cls.runner = execute.build_runner(harness.ROOT)[0]

    def round(self, extra):
        ScriptedAgent.files = reference_files("small", 1)
        ScriptedAgent.requests = 0
        ScriptedAgent.stream_seen = []
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), ScriptedAgent)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp, True)
        catalog = os.path.join(tmp, "api.json")
        with open(catalog, "w", encoding="utf-8") as handle:
            json.dump({"opencode-go": {"models": {"mock-model": {
                "cost": {"input": 0.10, "output": 0.40, "cache_read": 0.01}, "limit": {"context": 262144}}}}}, handle)
        out = os.path.join(tmp, "result.json")
        # A real key must never reach the runner or the mock: the round runs keyless.
        environment = {k: v for k, v in os.environ.items() if k != "OPENCODE_KEY"}
        with mock.patch.dict(os.environ, environment, clear=True):
            code = self.main(server, catalog, out, extra)
        self.assertEqual(code, 0)
        with open(out, encoding="utf-8") as handle:
            return json.load(handle)

    def main(self, server, catalog, out, extra):
        return harness.main(["run", "--sizes", "small", "--budgets", "200000", "--trials", "1", "--seed", "1",
                             "--model", "mock-model", "--via", "runner", "--runner", self.runner,
                             "--catalog", catalog, "--base-url", f"http://127.0.0.1:{server.server_address[1]}",
                             "--max-turns", "40", "--timeout", "600", "--out", out, *extra])

    def test_one_small_arm_passes_and_its_usage_and_spend_are_recorded(self):
        result = self.round([])
        self.assertEqual(result["errors"], [], result)
        self.assertEqual(len(result["rows"]), 1)
        row = result["rows"][0]
        self.assertTrue(row["passed"], row)
        self.assertEqual((row["stages_passed"], row["desks_passed"], row["failure"]), (3, 6, None))
        self.assertGreater(row["own_tests"]["count"], 0)
        self.assertEqual(row["requests"], ScriptedAgent.requests)
        self.assertEqual(row["input_tokens"], 1000 * row["requests"])
        self.assertEqual(row["output_tokens"], 100 * row["requests"])
        self.assertEqual(row["reasoning_output_tokens"], 20 * row["requests"])
        self.assertEqual(row["cached_share"], 0.6)
        per_request = (400 * 0.10 + 600 * 0.01 + 100 * 0.40) / 1_000_000
        self.assertAlmostEqual(row["cost_usd"], per_request * row["requests"], places=4)
        self.assertAlmostEqual(result["spend"]["spent_usd"], per_request * row["requests"], places=4)
        self.assertEqual(row["calls"], len(ScriptedAgent.files))
        self.assertEqual(row["compactions"], 0)
        self.assertEqual(row["budget"], 200000)
        self.assertIn("200000", result["summary"]["small"])
        report = harness.markdown(result)
        self.assertIn("| small | 200000 | 1 | 100% ± 0%", report)
        self.assertTrue(re.search(r"Spend ≈ \$0\.0", report))

    def test_a_run_over_its_cost_limit_is_stopped_and_graded_as_it_stands(self):
        result = self.round(["--max-run-cost", "0.0002"])
        row = result["rows"][0]
        self.assertEqual(row["failure"], "stopped: cost")
        self.assertFalse(row["passed"])
        self.assertLess(row["requests"], ScriptedAgent.requests + 1)
        self.assertLess(ScriptedAgent.requests, len(ScriptedAgent.files) + 3)
        self.assertIn("stages", row)


# A stand-in for the `themis` CLI with the commands `run_app` uses. `thread send --json` prints
# the app's event lines; the first message writes the files listed in the file named by `files`, one tool
# call each, with a context_usage event after each. `thread stop` makes a running send fail.
FAKE_THEMIS = r'''#!{python}
import json, os, sys, time
args = sys.argv[1:]
data = args[1]
words = args[2:]
with open({env_seen!r}, "w") as handle:
    json.dump(dict(os.environ), handle)
state = os.path.join(data, "fake.json")
stop = os.path.join(data, "stopped")
def out(value):
    print(json.dumps(value), flush=True)
if words[:2] == ["server", "start"] or words[:2] == ["server", "stop"] or words[0] == "call":
    out({{}})
elif words[:2] == ["thread", "create"]:
    with open(state, "w") as handle:
        json.dump({{"project": words[2], "requests": 0}}, handle)
    out({{"id": "thread-1"}})
elif words[:2] == ["thread", "stop"]:
    open(stop, "w").close()
    out({{}})
elif words[:2] == ["thread", "send"]:
    with open(state) as handle:
        saved = json.load(handle)
    text = words[3]
    files = json.load(open({files!r})) if "Implement `settle" in text else []
    def event(kind, **fields):
        out({{"name": "thread-event", "payload": {{"thread_id": "thread-1", "run_id": "r",
                                                  "event": dict(fields, kind=kind)}}}})
    requests = 0
    for path, body in files + [(None, None)]:
        if os.path.exists(stop):
            event("failed", error="Run stopped by you")
            sys.exit(1)
        requests += 1
        event("context_usage", metrics={{"total_requests": requests, "provider_input_tokens": 1000 * requests,
              "provider_output_tokens": 100 * requests, "provider_cached_input_tokens": 600 * requests,
              "provider_reasoning_output_tokens": 0, "estimated_input_tokens": 1000 * requests,
              "requests_missing_input_usage": 0}})
        time.sleep(0.2)
        if path is None:
            break
        event("tool_started", tool="write_file", summary=json.dumps({{"file_path": path}}))
        full = os.path.join(saved["project"], path)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "w") as handle:
            handle.write(body)
    event("finished", result="Done.")
'''


class AppPathTests(unittest.TestCase):
    """The app path's plumbing against a stand-in CLI: events, usage, the cost stop."""

    def round(self, extra):
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp, True)
        files = os.path.join(tmp, "files.json")
        self.env_seen = os.path.join(tmp, "env.json")
        with open(files, "w", encoding="utf-8") as handle:
            json.dump(reference_files("small", 1), handle)
        binary = os.path.join(tmp, "themis")
        with open(binary, "w", encoding="utf-8") as handle:
            handle.write(FAKE_THEMIS.format(python=sys.executable, files=files, env_seen=self.env_seen))
        os.chmod(binary, 0o755)
        catalog = os.path.join(tmp, "api.json")
        with open(catalog, "w", encoding="utf-8") as handle:
            json.dump({"opencode-go": {"models": {"m": {"cost": {"input": 0.10, "output": 0.40, "cache_read": 0.01},
                                                         "limit": {"context": 262144}}}}}, handle)
        out = os.path.join(tmp, "result.json")
        with mock.patch.dict(os.environ, {"UNRELATED_SECRET": "x"}), \
                mock.patch.object(spend, "preflight", return_value=(True, "")):
            code = harness.main(["run", "--sizes", "small", "--budgets", "128000", "--trials", "1", "--model", "m",
                                 "--via", "app", "--themis", binary, "--catalog", catalog, "--timeout", "300",
                                 "--out", out, *extra])
        self.assertEqual(code, 0)
        with open(out, encoding="utf-8") as handle:
            return json.load(handle)

    def test_a_passing_chat_records_its_calls_and_usage(self):
        row = self.round([])["rows"][0]
        self.assertTrue(row["passed"], row)
        self.assertEqual(row["via"], "app")
        self.assertEqual(row["calls"], len(reference_files("small", 1)))
        self.assertEqual(row["cached_share"], 0.6)
        self.assertEqual(row["input_tokens"], 1000 * row["requests"])

    def test_the_app_gets_only_a_minimal_environment(self):
        self.round([])
        with open(self.env_seen, encoding="utf-8") as handle:
            seen = json.load(handle)
        self.assertNotIn("UNRELATED_SECRET", seen)
        self.assertTrue(seen["HOME"].startswith(tempfile.gettempdir()) or "budget-app-" in seen["HOME"], seen["HOME"])

    def test_the_chat_is_stopped_when_the_run_passes_its_cost_limit(self):
        row = self.round(["--max-run-cost", "0.0002"])["rows"][0]
        self.assertEqual(row["failure"], "stopped: cost")
        self.assertLess(row["calls"], len(reference_files("small", 1)))

    def test_no_run_starts_when_its_estimate_would_pass_the_round_limit(self):
        # The prior for a small run (≈ $0.072 at these prices) is above a $0.05 round.
        result = self.round(["--max-round-cost", "0.05"])
        self.assertEqual(result["rows"], [])
        self.assertEqual([n["why"] for n in result["not_run"]], ["round cost limit"])
        self.assertEqual(result["spend"]["spent_usd"], 0)

    def test_budgets_above_the_app_limit_need_the_runner(self):
        with tempfile.TemporaryDirectory() as tmp:
            catalog = os.path.join(tmp, "api.json")
            with open(catalog, "w", encoding="utf-8") as handle:
                json.dump({"opencode-go": {"models": {"m": {"cost": {"input": 0}, "limit": {"context": 1_000_000}}}}},
                          handle)
            code = harness.main(["run", "--model", "m", "--catalog", catalog, "--via", "app",
                                 "--budgets", str(harness.app_budget_limit() + 1)])
            self.assertEqual(code, 2)


if __name__ == "__main__":
    unittest.main()
