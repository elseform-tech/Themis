"""Offline self-tests for the goal soak: the generator, every grader on synthetic event streams
(a clean run and one run per planted breakage), the trend logic, and the runner end to end
against a stand-in `themis` CLI (`fake_themis.py`; no model, no network, no spend).

    python3 -m unittest evals.soak.test_harness
"""

import contextlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

from evals.kit.spend import Price

from . import generate as gen
from . import tool_causes, trend
from .grade import GOAL_TURN_MARKER, MEASURES, grade, infrastructure
from .run import Soak, checkpoint_summary

HERE = os.path.dirname(os.path.abspath(__file__))


# --- synthetic traces ---------------------------------------------------------------------------

class TraceBuilder:
    """Builds a trace like the runner records: one goal turn per item, a checkpoint every
    `every` turns, the follow-up after half the items, reached at the end, three recall answers."""

    def __init__(self, spec, every=3):
        self.spec, self.every = spec, every
        self.events, self.t = [], 0.0
        self.messages = gen.messages(spec)
        self.goal_text = self.messages["goal"][len("/goal"):].strip()

    def emit(self, run_id, kind, **fields):
        self.t += 1
        self.events.append({"t": self.t, "run_id": run_id, "event": {"kind": kind, **fields}})

    def tool(self, run_id, tool, summary, ok=True, output="ok"):
        self.emit(run_id, "tool_started", tool=tool, summary=summary)
        self.emit(run_id, "tool_finished", tool=tool, ok=ok, output=output)

    def checkpoint(self, run_id, written, before, after, since=False):
        self.emit(run_id, "context_budget", budget={"used_tokens": before})
        self.emit(run_id, "context_compacting")
        self.emit(run_id, "context_checkpoint", summary=(
            "Goal: write every out/<name>.md, then out/INDEX.md, run python3 check.py, commit. Rules from the "
            "first message: every item file starts with `owner: <team>`; retired items are listed in the index "
            f"but get no file. Done: {', '.join(written)}." + (" Every file now has a since line." if since else "")))
        self.emit(run_id, "context_budget", budget={"used_tokens": after})

    def build(self):
        items = self.spec["items"]
        active = [item for item in items if item["status"] != "retired"]
        files, written, since = {}, [], False
        self.emit("r0", "user_message", text=self.messages["rules"])
        self.emit("r0", "finished", result="Rules noted.")
        half = len(active) // 2
        for turn, item in enumerate(active, start=1):
            run = f"g{turn}"
            text = self.messages["goal"] if turn == 1 else f"{GOAL_TURN_MARKER}\nTurn {turn}: continue"
            self.emit(run, "user_message", text=text)
            self.emit(run, "context_usage", metrics={"total_requests": 2, "provider_input_tokens": 9000,
                                                     "provider_output_tokens": 300})
            self.tool(run, "read_file", f"read `src/{item['name']}.txt`")
            self.tool(run, "write_file", f"write `out/{item['name']}.md`")
            files[f"out/{item['name']}.md"] = gen.output_text(item, since)
            written.append(item["name"])
            if turn % self.every == 0:
                self.checkpoint(run, written, before=20000, after=6000, since=since)
            self.emit(run, "finished", result=f"Wrote {item['name']}.")
            if turn == half:
                self.emit("q1", "user_message", text=self.messages["followup"])
                since = True
                for name in written:
                    self.tool("q1", "apply_patch", f"call `apply_patch` with --- a/out/{name}.md +++ b/out/{name}.md")
                    files[f"out/{name}.md"] = gen.output_text(next(i for i in items if i["name"] == name), True)
                self.emit("q1", "finished", result="Updated.")
            if turn == len(active):
                self.tool(run, "shell", "call `shell` with python3 check.py", output="OK")
                files["out/INDEX.md"] = gen.index_text(self.spec)
                self.emit("", "goal_changed", goal={"text": self.goal_text, "status": "reached"})
                # The app's own short turn: the agent tells the user the goal is reached.
                self.emit("gr", "user_message", text=f"{GOAL_TURN_MARKER} Turn {turn + 1}. Goal: {self.goal_text}\n"
                                                     "A separate check found the goal reached: check passes")
                self.emit("gr", "finished", result="I've reached the goal: check.py passes.")
        recall = []
        for number, question in enumerate(self.messages["recall"]):
            run = f"a{number}"
            answer = {"early_fact": str(items[0]["count"]), "rule": "owner: <team>",
                      "count": str(sum(i["status"] == "retired" for i in items))}[question["id"]]
            self.emit(run, "user_message", text=question["text"])
            self.emit(run, "finished", result=answer)
            recall.append({"id": question["id"], "expect": question["expect"], "run_id": run, "answer": answer,
                           "tools": 0})
        polls = [{"t": t, "phase": "goal", "goal": {"text": self.goal_text, "status": "running"}} for t in (5, 10, 15)]
        return {"spec": self.spec, "budget": 24000, "seed": self.spec["seed"], "model": "cheap-model", "mode": "yolo",
                "events": self.events, "goal_polls": polls, "saved_checkpoints": [], "goal_text": self.goal_text,
                "followup": {"t": 0, "text": self.messages["followup"]}, "recall": recall,
                "end": {"reason": "reached", "detail": ""},
                "final": {"files": files, "check_passed": True, "commits": 2, "dirty": False},
                "usage": {"requests": 40, "input_tokens": 200000, "cached_input_tokens": 50000, "output_tokens": 4000},
                "cost": 0.05, "wall_s": 1500}


def spec(items=12, seed=3):
    return gen.spec_for(seed, items)


def clean():
    return TraceBuilder(spec()).build()


def harness_flags(measures):
    return {name for name, value in measures.items()
            if MEASURES[name][1] == "harness" and value is not None and value > 0}


# --- generator ----------------------------------------------------------------------------------

class GeneratorTests(unittest.TestCase):
    def setUp(self):
        self.dirs = [tempfile.mkdtemp() for _ in range(3)]

    def tearDown(self):
        for path in self.dirs:
            shutil.rmtree(path, ignore_errors=True)

    def tree(self, path):
        found = {}
        for folder, _dirs, files in os.walk(path):
            for name in files:
                with open(os.path.join(folder, name), encoding="utf-8") as handle:
                    found[os.path.relpath(os.path.join(folder, name), path)] = handle.read()
        return found

    def test_same_seed_same_project_and_other_seeds_differ(self):
        first = gen.generate(7, self.dirs[0], items=20)
        again = gen.generate(7, self.dirs[1], items=20)
        other = gen.generate(8, self.dirs[2], items=20)
        self.assertEqual(first, again)
        self.assertEqual(self.tree(self.dirs[0]), self.tree(self.dirs[1]))
        self.assertNotEqual(first["items"], other["items"])

    def test_sizes_names_and_statuses(self):
        for items in (6, 40, 120):
            made = gen.spec_for(1, items)
            names = [item["name"] for item in made["items"]]
            self.assertEqual(len(names), items)
            self.assertEqual(len(set(names)), items)
            self.assertEqual(names, sorted(names))
            for status in gen.STATUSES:
                self.assertGreaterEqual(sum(item["status"] == status for item in made["items"]), 2)
        with self.assertRaises(ValueError):
            gen.spec_for(1, 5)
        gen.generate(1, self.dirs[0])
        tokens = gen.material_tokens(self.dirs[0])
        # The defaults: 120 memos of about 1,650 tokens each (see the README's size estimate).
        self.assertTrue(180_000 < tokens < 215_000, tokens)
        # Every memo fits one read_file call (8,000 characters).
        for name in os.listdir(os.path.join(self.dirs[0], "src")):
            self.assertLess(os.path.getsize(os.path.join(self.dirs[0], "src", name)), 8000, name)

    def test_the_checker_memos_and_format_are_read_only(self):
        import stat

        from evals.soak.run import protect
        gen.generate(3, self.dirs[0], items=6)
        protect(self.dirs[0])
        for name in ("check.py", "FORMAT.md", os.path.join("src", sorted(os.listdir(os.path.join(self.dirs[0], "src")))[0])):
            mode = os.stat(os.path.join(self.dirs[0], name)).st_mode
            self.assertFalse(mode & (stat.S_IWUSR | stat.S_IWGRP | stat.S_IWOTH), name)

    def test_rules_live_only_in_the_first_message(self):
        made = gen.generate(2, self.dirs[0], items=10)
        script = gen.messages(made)
        self.assertIn("owner: <team>", script["rules"])
        self.assertIn("retired", script["rules"])
        for text in (script["goal"], script["followup"], self.tree(self.dirs[0])["FORMAT.md"]):
            self.assertNotIn("owner", text)
            self.assertNotRegex(text.lower(), r"no file|never get|without (?:a|its own) file")
        for text in (script["goal"], script["followup"]):
            self.assertNotIn("retired", text.lower())
        self.assertTrue(script["goal"].startswith("/goal "))

    def test_memos_state_no_fact_as_a_key_value_line(self):
        made = gen.generate(5, self.dirs[0], items=20)
        for item in made["items"]:
            text = self.tree(self.dirs[0])[os.path.join("src", f"{item['name']}.txt")]
            self.assertNotRegex(text, r"(?m)^(?:owner|status|count|purpose|since)\s*:")
            self.assertIn(item["owner"], text)
            self.assertIn(str(item["since"]), text)
        self.assertEqual(gen.words(412), "four hundred and twelve")
        self.assertEqual(gen.words(1905), "one thousand nine hundred and five")
        self.assertEqual(gen.words(1040), "one thousand and forty")
        self.assertEqual(gen.words(70), "seventy")

    def test_check_checks_structure_and_total_but_reveals_no_fact(self):
        made = gen.generate(4, self.dirs[0], items=10)
        out = os.path.join(self.dirs[0], "out")
        os.makedirs(out)

        def check():
            return subprocess.run([sys.executable, "check.py"], cwd=self.dirs[0], capture_output=True, text=True)
        self.assertEqual(check().returncode, 1)
        for item in made["items"]:
            if item["status"] != "retired":
                with open(os.path.join(out, f"{item['name']}.md"), "w", encoding="utf-8") as handle:
                    handle.write(gen.output_text(item))
        with open(os.path.join(out, "INDEX.md"), "w", encoding="utf-8") as handle:
            handle.write(gen.index_text(made))
        self.assertEqual(check().returncode, 0, check().stdout)
        # Wrong facts in an item file pass (check.py cannot know them); the since line is allowed.
        item = next(i for i in made["items"] if i["status"] != "retired")
        with open(os.path.join(out, f"{item['name']}.md"), "w", encoding="utf-8") as handle:
            handle.write(gen.output_text(dict(item, count=item["count"] + 1, status="paused"), since=True))
        self.assertEqual(check().returncode, 0, check().stdout)
        # A wrong total, a count in words or an unknown status fails.
        with open(os.path.join(out, "INDEX.md"), "w", encoding="utf-8") as handle:
            handle.write(gen.index_text(made).replace(f"total: {gen.total(made)}", f"total: {gen.total(made) + 1}"))
        self.assertIn("the total is not the sum", check().stdout)
        with open(os.path.join(out, f"{item['name']}.md"), "w", encoding="utf-8") as handle:
            handle.write(gen.output_text(item).replace(f"count: {item['count']}", "count: many")
                         .replace(f"status: {item['status']}", "status: sleeping"))
        result = check().stdout
        self.assertIn("must be a number in digits", result)
        self.assertIn("must be one of", result)
        # The checker holds no fact: not the total, a count or an owner.
        source = self.tree(self.dirs[0])["check.py"]
        self.assertNotIn(str(gen.total(made)), source)
        for team in gen.TEAMS:
            self.assertNotIn(team, source)
        for other in made["items"]:
            self.assertNotRegex(source, rf"\b{other['count']}\b")

    def test_prompt_files_have_no_trailing_newline_and_fill_placeholders(self):
        for name in os.listdir(gen.PROMPTS):
            with open(os.path.join(gen.PROMPTS, name), encoding="utf-8") as handle:
                self.assertFalse(handle.read().endswith("\n"), name)
        with self.assertRaises(KeyError):
            gen.prompt("recall-fact")


class ScriptShortcutTests(unittest.TestCase):
    """A script over the memos (the pilot's agent wrote one) gets most items wrong."""

    STATUS = (("active", r"running|in service|daily use|went live|without interruption"),
              ("paused", r"on hold|suspended|idle"), ("retired", r"shut down|decommissioned|switched off|recycled"))

    def extract(self, text, pick, targeted):
        if targeted:
            # Phrase-aware: the team after "to"/"belongs to"/"keepers are", the number after
            # "holds"/"total is", the purpose after "keep the".
            teams = re.findall(r"(?:to|belongs to|keepers are|of|by) (team-[a-z]+)", text)
            numbers = [int(n) for n in re.findall(r"(?:holds|total is|other) (\d+)", text)]
            purposes = re.findall(r"keep(?:s)? the ([a-z]+)", text)
        else:
            teams = re.findall(r"team-[a-z]+", text)
            numbers = [int(n) for n in re.findall(r"\b\d+\b", text) if not 1990 < int(n) < 2030]
            purposes = [word for word in re.findall(r"[a-z]+", text) if word in gen.PURPOSES]
        found = sorted((m.start(), status) for status, pattern in self.STATUS for m in re.finditer(pattern, text))
        return {"owner": pick(teams), "count": pick(numbers), "purpose": pick(purposes),
                "status": pick([status for _at, status in found])}

    def test_regex_extractors_get_most_items_wrong(self):
        folder = tempfile.mkdtemp()
        try:
            made = gen.generate(1, folder, items=60)
            for targeted in (False, True):
                for pick in (lambda found: found[0] if found else None, lambda found: found[-1] if found else None):
                    right = 0
                    for item in made["items"]:
                        with open(os.path.join(folder, "src", f"{item['name']}.txt"), encoding="utf-8") as handle:
                            got = self.extract(handle.read(), pick, targeted)
                        right += all(got[key] == item[key] for key in ("owner", "count", "purpose", "status"))
                    self.assertLess(right / len(made["items"]), 0.2, f"targeted={targeted}")
            # A correct reading passes the grader's checks for every item.
            for item in made["items"]:
                self.assertTrue(gen.item_correct(item, gen.output_text(item)))
                self.assertTrue(gen.owner_correct(item, gen.output_text(item)))
                self.assertTrue(gen.since_correct(item, gen.output_text(item, since=True)))
            self.assertEqual(gen.index_statuses(made, gen.index_text(made)), 1.0)
        finally:
            shutil.rmtree(folder)

    def test_item_checks_are_exact(self):
        item = gen.spec_for(1, 10)["items"][0]
        good = gen.output_text(item)
        self.assertFalse(gen.item_correct(item, good.replace(f"count: {item['count']}", f"count: {item['count'] + 1}")))
        self.assertFalse(gen.item_correct(item, good.replace(item["purpose"], "nothing")))
        self.assertFalse(gen.item_correct(item, None))
        self.assertFalse(gen.owner_correct(item, good.replace(item["owner"], "team-zzz")))
        self.assertFalse(gen.since_correct(item, good))


# --- graders ------------------------------------------------------------------------------------

class GraderTests(unittest.TestCase):
    def test_clean_run_has_no_breakage(self):
        trace = clean()
        measures, details = grade(trace, trace["spec"])
        self.assertEqual(harness_flags(measures), set())
        self.assertEqual(measures["reached"], 1)
        self.assertEqual(measures["checkpoints"], 3)
        self.assertEqual(measures["rule_retention"], 1.0)
        self.assertEqual(measures["redone_items"], 0)
        self.assertEqual(measures["skipped_items"], 0)
        self.assertEqual(measures["duplicated_items"], 0)
        self.assertEqual(measures["rereads_after_checkpoint"], 0)
        self.assertEqual(measures["rule_violations_after_checkpoint"], 0)
        self.assertEqual(measures["format_change_applied"], 1.0)
        self.assertEqual((measures["item_accuracy"], measures["owner_accuracy"], measures["index_accuracy"]),
                         (1.0, 1.0, 1.0))
        self.assertEqual(measures["recall_correct"], 3)
        self.assertEqual(measures["cached_share"], 0.25)
        self.assertEqual(details["end_state"], "reached")
        self.assertFalse(infrastructure(trace))
        # Checkpoints after the follow-up must also keep the format change.
        self.assertIn("format_change", details["retention"][-1]["kept"])

    def test_goal_not_continuing(self):
        trace = clean()
        cut = next(i for i, item in enumerate(trace["events"]) if item["run_id"] == "g2")
        trace["events"] = trace["events"][:cut]
        trace["end"] = {"reason": "not_continuing", "detail": "the goal runs but no turn started"}
        measures, _details = grade(trace, trace["spec"])
        self.assertEqual(measures["goal_continuation_breaks"], 1)
        self.assertEqual(measures["health_breakage"], 1)
        # A cap that cuts the last turn short is not a break.
        trace["end"] = {"reason": "cap_cost", "detail": ""}
        measures, _details = grade(trace, trace["spec"])
        self.assertEqual(measures["goal_continuation_breaks"], 0)
        self.assertEqual(measures["health_breakage"], 0)

    def test_extra_turn_after_reached(self):
        trace = clean()
        told = max(i for i, item in enumerate(trace["events"]) if item["run_id"] == "gr")
        extra = {"t": 0, "run_id": "g99", "event": {"kind": "user_message", "text": f"{GOAL_TURN_MARKER}\nTurn 99"}}
        trace["events"].insert(told + 1, extra)
        measures, _details = grade(trace, trace["spec"])
        self.assertEqual(harness_flags(measures), {"turns_after_reached"})
        # Neither the app's reached turn nor recall questions sent after it are goal turns.
        self.assertEqual(grade(clean(), clean()["spec"])[0]["turns_after_reached"], 0)

    def test_failed_checkpoint(self):
        for planted in ("suspended", "budget_error", "compacting_then_failed"):
            trace = clean()
            run = [{"t": 0, "run_id": "gx", "event": {"kind": "context_compacting"}}]
            if planted == "suspended":
                run.append({"t": 0, "run_id": "gx", "event": {"kind": "context_suspended", "progress": "..."}})
            elif planted == "budget_error":
                run.append({"t": 0, "run_id": "gx", "event": {"kind": "failed", "error": (
                    "checkpoint and active request exceed the context budget; original conversation preserved")}})
            else:
                run.append({"t": 0, "run_id": "gx", "event": {"kind": "failed", "error": "summary was empty"}})
            trace["events"][5:5] = run
            measures, _details = grade(trace, trace["spec"])
            self.assertEqual(measures["failed_checkpoints"], 1, planted)
            self.assertIn("failed_checkpoints", harness_flags(measures))

    def test_context_not_shrinking(self):
        trace = clean()
        first = next(i for i, item in enumerate(trace["events"]) if item["event"]["kind"] == "context_checkpoint")
        trace["events"][first + 1]["event"]["budget"]["used_tokens"] = 21000
        measures, details = grade(trace, trace["spec"])
        self.assertEqual(measures["checkpoints_not_shrinking"], 1)
        self.assertEqual(harness_flags(measures), {"checkpoints_not_shrinking"})
        self.assertEqual(details["checkpoint_sizes"][0], (20000, 21000))

    def test_redo_after_checkpoint_is_counted_and_its_checkpoint_share_shown(self):
        trace = clean()
        first = next(i for i, item in enumerate(trace["events"]) if item["event"]["kind"] == "context_checkpoint")
        done = trace["spec"]["items"]
        name = next(i["name"] for i in done if i["status"] != "retired")
        redo = [{"t": 0, "run_id": "g3", "event": {"kind": "tool_started", "tool": "write_file",
                                                   "summary": f"write `out/{name}.md`"}},
                {"t": 0, "run_id": "g3", "event": {"kind": "tool_finished", "tool": "write_file", "ok": True,
                                                   "output": "ok"}},
                {"t": 0, "run_id": "g3", "event": {"kind": "tool_started", "tool": "read_file",
                                                   "summary": f"read `src/{name}.txt`"}},
                {"t": 0, "run_id": "g3", "event": {"kind": "tool_finished", "tool": "read_file", "ok": True,
                                                   "output": "..."}}]
        trace["events"][first + 2:first + 2] = redo
        measures, _details = grade(trace, trace["spec"])
        self.assertEqual(measures["redone_items"], 1)
        # The checkpoint's saved progress listed the item: reported as the checkpoint's share.
        self.assertEqual(measures["redone_checkpoint_share"], 1)
        self.assertEqual(measures["rereads_after_checkpoint"], 1)
        self.assertEqual(harness_flags(measures), set())

    def test_rules_broken_after_a_checkpoint_and_lost_from_summaries(self):
        trace = clean()
        active = [i for i in trace["spec"]["items"] if i["status"] != "retired"]
        last = active[-1]
        trace["final"]["files"][f"out/{last['name']}.md"] = gen.output_text(last, since=True).split("\n", 1)[1]
        # A wrong team on the owner line is a misreading, not a forgotten rule.
        misread = active[-2]
        trace["final"]["files"][f"out/{misread['name']}.md"] = gen.output_text(misread, since=True).replace(
            misread["owner"], "team-zzz")
        retired = next(i for i in trace["spec"]["items"] if i["status"] == "retired")
        trace["final"]["files"][f"out/{retired['name']}.md"] = gen.output_text(retired)
        trace["saved_checkpoints"] = ["Goal: out/INDEX.md and check.py. Done: three items.",
                                      "Goal: out/INDEX.md and check.py. Every item file starts with owner: <team>; "
                                      "retired items get no file."]
        measures, details = grade(trace, trace["spec"])
        self.assertEqual(measures["rule_violations_after_checkpoint"], 1)
        self.assertEqual(details["final_rule_violations"], 2)
        self.assertLess(measures["owner_accuracy"], 1.0)
        self.assertEqual(measures["duplicated_items"], 0)
        self.assertEqual(details["retention"][0]["kept"], {"goal": True, "rule_owner": False, "rule_retired": False})
        self.assertAlmostEqual(measures["rule_retention_min"], 1 / 3, places=3)

    def test_tool_internal_error_is_ours(self):
        trace = clean()
        trace["events"][3:3] = [
            {"t": 0, "run_id": "g1", "event": {"kind": "tool_started", "tool": "shell", "summary": "call `shell`"}},
            {"t": 0, "run_id": "g1", "event": {"kind": "tool_finished", "tool": "shell", "ok": False,
                                               "output": "shell: execution task failed: JoinError::Panic"}},
            {"t": 0, "run_id": "g1", "event": {"kind": "tool_started", "tool": "read_file", "summary": "read `x`"}},
            {"t": 0, "run_id": "g1", "event": {"kind": "tool_finished", "tool": "read_file", "ok": False,
                                               "output": "read_file: path 'src/nope.txt' does not exist"}},
            {"t": 0, "run_id": "g1", "event": {"kind": "tool_started", "tool": "git", "summary": "call `git`"}},
            {"t": 0, "run_id": "g1", "event": {"kind": "tool_finished", "tool": "git", "ok": False,
                                               "output": "git: something odd happened"}}]
        measures, details = grade(trace, trace["spec"])
        self.assertEqual((measures["tool_errors_ours"], measures["tool_errors_model"],
                          measures["tool_errors_unclassified"]), (1, 1, 1))
        self.assertEqual(details["ours"][0]["code"], "internal")
        self.assertEqual(details["unclassified"][0]["tool"], "git")
        self.assertEqual(harness_flags(measures), {"tool_errors_ours", "tool_errors_unclassified"})

    def test_failed_runs_and_infrastructure(self):
        trace = clean()
        trace["events"].append({"t": 0, "run_id": "gz", "event": {"kind": "failed", "error": "Reached max turns"}})
        self.assertEqual(grade(trace, trace["spec"])[0]["failed_runs"], 1)
        lost = clean()
        lost["end"] = {"reason": "failed", "detail": ""}
        lost["events"].append({"t": 0, "run_id": "gz", "event": {
            "kind": "failed", "error": "LLM request failed: HTTP 429 Too Many Requests"}})
        self.assertEqual(grade(lost, lost["spec"])[0]["failed_runs"], 0)
        self.assertTrue(infrastructure(lost))
        # A provider failure the goal recovered from (later turns ran) does not lose the run.
        recovered = clean()
        recovered["end"] = {"reason": "stuck", "detail": ""}
        recovered["events"].insert(0, {"t": 0, "run_id": "early", "event": {
            "kind": "failed", "error": "LLM request failed: Generic Error : Provider response failed or was incomplete"}})
        self.assertFalse(infrastructure(recovered))
        stopped = clean()
        stopped["events"].append({"t": 0, "run_id": "gz", "event": {"kind": "failed", "error": "Stopped by you"}})
        self.assertEqual(grade(stopped, stopped["spec"])[0]["failed_runs"], 0)

    def test_goal_changed(self):
        trace = clean()
        trace["goal_polls"].append({"t": 20, "phase": "goal", "goal": {"text": "something else", "status": "running"}})
        trace["goal_polls"].append({"t": 25, "phase": "after", "goal": None})
        self.assertEqual(grade(trace, trace["spec"])[0]["goal_changed"], 1)
        early = clean()
        early["goal_polls"].insert(0, {"t": 1, "phase": "goal", "goal": None})
        self.assertEqual(grade(early, early["spec"])[0]["goal_changed"], 0)
        early["goal_polls"].append({"t": 30, "phase": "goal", "goal": None})
        self.assertEqual(grade(early, early["spec"])[0]["goal_changed"], 1)

    def test_recall_from_tools_does_not_count(self):
        trace = clean()
        trace["recall"][0]["tools"] = 1
        trace["recall"][1]["answer"] = "I do not remember"
        self.assertEqual(grade(trace, trace["spec"])[0]["recall_correct"], 1)

    def test_tool_causes(self):
        cases = [("read_file", "not found", "ours"),
                 ("write_file", "approval denied for 'write_file': write out/a.md", "ours"),
                 ("apply_patch", "apply_patch: patch does not apply to 'out/a.md': context mismatch near line 2", "model"),
                 ("read_file", "read_file: argument 'file_path' is invalid: invalid type", "model"),
                 ("bogus", "unknown tool 'bogus'. (available: read_file)", "model"),
                 ("shell", "Traceback (most recent call last): ZeroDivisionError", "model"),
                 ("read_file", "path '../x' escapes the project root", "model"),
                 # From the first live pilot: a JSONL field asked of a memo; the text "null" as cwd.
                 ("read_file", "tool 'read_file' failed: Selected JSONL field is missing or is not a string", "model"),
                 ("shell", "tool 'shell' failed: shell: working directory '/tmp/soak/project/null' is not a directory",
                  "model"),
                 # From the first 64k round: reading past a record's end; an absolute path.
                 ("read_file", "tool 'read_file' failed: Read offset 1000 is past the end: the text has 0 characters, "
                               "so everything was already read", "model"),
                 ("read_file", "tool 'read_file' failed: Absolute paths are not allowed inside root-scoped tools: /x",
                  "model"),
                 # From the round after goals stopped giving up: a non-web URL for the browser.
                 ("browser_open", "tool 'browser_open' failed: browser_open: only http and https URLs are supported, "
                                  "not about:", "model"),
                 ("web_search", "web_search: the moon is in the way", "unclassified")]
        for tool, text, cause in cases:
            self.assertEqual(tool_causes.classify(tool, text)[0], cause, text)
        # Custom mode (the soak's rules): denying an allowed tool or the check command is ours;
        # denying anything else is the model reaching past the rules.
        denied = "approval denied for '{}': call `{}` with {} The action was refused."
        custom = [("write_file", denied.format("write_file", "write_file", '{"file_path":"out/a.md"}'), "ours"),
                  ("shell", denied.format("shell", "shell", '{"command":"python3","args":["check.py"],"cwd":""}'), "ours"),
                  ("shell", denied.format("shell", "shell", '{"command":"sh","args":["-c","python3 x.py"]}'), "model"),
                  # grep that prints only names or counts is allowed; one that prints contents is not.
                  ("shell", denied.format("shell", "shell", '{"command":"grep","args":["-rL","^- since:","out"]}'), "ours"),
                  ("shell", denied.format("shell", "shell", '{"command":"grep","args":["-r","owner","src"]}'), "model"),
                  ("search_file", denied.format("search_file", "search_file", '{"pattern":"owner"}'), "model"),
                  # A list sent as a JSON string is read as that list, as Themis does.
                  ("shell", denied.format("shell", "shell", r'{"command":"grep","args":"[\"-L\", \"x\", \"out\"]"}'),
                   "ours"),
                  ("shell", denied.format("shell", "shell", r'{"command":"grep","args":"[\"-n\", \"x\"]"}'), "model")]
        for tool, text, cause in custom:
            summary = "call `{}` with {}".format(tool, text.split(" with ", 1)[1].rsplit(" The action", 1)[0])
            self.assertEqual(tool_causes.classify(tool, text, mode="custom", summary=summary)[0], cause, text)
        # A shell call whose arguments cannot be parsed is left for a person, not attributed.
        self.assertEqual(tool_causes.classify("shell", custom[1][1], mode="custom", summary="call `shell` with ???")[0],
                         "unclassified")
        # Summaries are cut short: a complete command or first argument that differs from the
        # check command is enough; a cut that leaves the check command possible is not.
        shell_denied = denied.format("shell", "shell", "{...}")
        for summary, cause in (('call `shell` with {"command":"sh","args":["-c","echo SRC=$(ls src | wc', "model"),
                               ('call `shell` with {"command":"python3","args":["-c","import os,subpro', "model"),
                               ('call `shell` with {"command":"python3","args":["check.py"', "unclassified"),
                               ('call `shell` with {"command":"grep","args":["-c"', "unclassified"),
                               ('call `shell` with {"command":"grep","args":["-n","x', "model"),
                               ('call `shell` with {"command":"pyth', "unclassified"),
                               ('call `shell` with {"args":["x"],"command":"sh', "unclassified")):
            self.assertEqual(tool_causes.classify("shell", shell_denied, mode="custom", summary=summary)[0], cause,
                             summary)
        # The soak makes check.py, FORMAT.md and the memos read-only: writing them is the model's.
        refused = "tool 'write_file' failed: Permission denied (os error 13)"
        for path, cause in (("check.py", "model"), ("./src/a-b.txt", "model"), ("out/a-b.md", "unclassified")):
            summary = 'call `write_file` with {"file_path":"%s","content":"x' % path
            self.assertEqual(tool_causes.classify("write_file", refused, mode="custom", summary=summary)[0], cause,
                             path)
        self.assertEqual(tool_causes.APPROVAL["rules"][0], {"tool": "shell_unsandboxed", "action": "deny"})


# --- trend --------------------------------------------------------------------------------------

def record(measures=None, model="cheap-model", budget=24000, seed=1, infra=False, index=0):
    base = grade(clean(), clean()["spec"])[0]
    base.update(measures or {})
    return {"id": f"r{index}-{model}-{budget}", "model": model, "budget": budget, "seed": seed, "job": gen.JOB,
            "size": 12, "infrastructure": infra, "measures": base,
            "details": {"end_state": "reached", "unclassified": [], "ours": [], "retention": [], "segments": {}}}


class TrendTests(unittest.TestCase):
    def test_fewer_than_five_runs_show_values_without_model_flags(self):
        history = [record({"redone_items": 0}, index=i) for i in range(4)]
        current = record({"redone_items": 9}, index=99)
        self.assertEqual(trend.flags(history, current), {})
        # A harness breakage is flagged however short the history.
        broken = record({"failed_checkpoints": 1}, index=98)
        self.assertEqual(trend.flags([], broken), {"failed_checkpoints": "breakage"})

    def test_regression_only_when_worse_than_all_five(self):
        history = [record({"redone_items": value}, index=i) for i, value in enumerate([0, 1, 0, 2, 1])]
        self.assertEqual(trend.flags(history, record({"redone_items": 3}, index=99)), {"redone_items": "regression"})
        self.assertEqual(trend.flags(history, record({"redone_items": 2}, index=99)), {})
        rows, count = trend.compare(history, record({"redone_items": 3}, index=99))
        self.assertEqual(count, 5)
        self.assertEqual((rows["redone_items"]["baseline"], rows["redone_items"]["low"], rows["redone_items"]["high"]),
                         (1, 0, 2))
        # Higher is better for retention: lower than every earlier run is a regression.
        history = [record({"rule_retention": value}, index=i) for i, value in enumerate([0.9, 1.0, 0.8, 0.9, 1.0])]
        self.assertEqual(trend.flags(history, record({"rule_retention": 0.7}, index=99)), {"rule_retention": "regression"})

    def test_baseline_uses_only_the_last_five(self):
        history = [record({"redone_items": 9}, index=i) for i in range(5)]
        history += [record({"redone_items": 0}, index=10 + i) for i in range(5)]
        self.assertEqual(trend.flags(history, record({"redone_items": 3}, index=99)), {"redone_items": "regression"})

    def test_series_split_and_infrastructure_left_out(self):
        history = [record({"redone_items": 0}, index=i) for i in range(5)]
        for other in (record({"redone_items": 0}, model="other-model", index=50),
                      record({"redone_items": 0}, budget="auto", index=51)):
            self.assertEqual(trend.compare(history, other)[1], 0)
        history = [record({"redone_items": 0}, index=i, infra=i < 2) for i in range(5)]
        self.assertEqual(trend.compare(history, record({"redone_items": 3}, index=99))[1], 3)
        self.assertEqual(trend.flags(history, record({"redone_items": 3}, index=99)), {})

    def test_record_in_history_compares_with_runs_before_it(self):
        history = [record({"redone_items": 0}, index=i) for i in range(6)]
        rows, count = trend.compare(history, history[2])
        self.assertEqual(count, 2)

    def test_history_round_trip_and_report(self):
        folder = tempfile.mkdtemp()
        try:
            path = os.path.join(folder, "history.jsonl")
            for i in range(5):
                trend.append(path, record({"redone_items": i % 2}, index=i))
            current = record({"redone_items": 4, "tool_errors_unclassified": 1}, index=9)
            current["details"]["unclassified"] = [{"tool": "git", "error": "something odd"}]
            trend.append(path, current)
            history = trend.load(path)
            self.assertEqual(len(history), 6)
            text = trend.report(history, [history[-1]])
            self.assertIn("**regression**", text)
            self.assertIn("**breakage**", text)
            self.assertLess(text.index("Unclassified tool errors"), text.index("| Area |"))
            self.assertIn("## Budget 24000", text)
        finally:
            shutil.rmtree(folder)

    def test_sparkline(self):
        self.assertEqual(trend.sparkline([0, 1, 2]), "▁▅█")
        self.assertEqual(trend.sparkline([3, 3]), "▄▄")
        self.assertEqual(trend.sparkline([None]), "")

    def test_every_measure_has_a_known_kind_and_direction(self):
        for name, (area, kind, better) in MEASURES.items():
            self.assertIn(area, trend.AREAS, name)
            self.assertIn(kind, ("harness", "model", "reported"), name)
            self.assertEqual(better is None, kind == "reported", name)


# --- the runner against a stand-in CLI ------------------------------------------------------------

@unittest.skipIf(os.name == "nt", "the stand-in CLI is a POSIX shell wrapper")
class RunnerTests(unittest.TestCase):
    OPTIONS = {"model": "cheap-model", "items": 8, "filler_chars": 200, "max_turns": 40, "timeout": 120,
               "idle_timeout": 2, "poll_s": 0.2, "grace_s": 0.5, "subscribe_s": 0.5, "max_run_cost": 1.5}

    def setUp(self):
        self.folder = tempfile.mkdtemp()
        self.binary = os.path.join(self.folder, "fake-themis")
        with open(self.binary, "w", encoding="utf-8") as handle:
            handle.write(f'#!/bin/sh\nexec "{sys.executable}" "{os.path.join(HERE, "fake_themis.py")}" "$@"\n')
        os.chmod(self.binary, 0o755)

    def tearDown(self):
        shutil.rmtree(self.folder, ignore_errors=True)

    def soak(self, mode, **options):
        soak = Soak(self.binary, dict(self.OPTIONS, **options), 24000, 1, Price(0.1, 0.4))
        original = soak._drive

        def drive(trace, script, started):
            # The stand-in agent cannot read prose: it is handed the true facts.
            with open(os.path.join(soak.data, "fake.json"), "w", encoding="utf-8") as handle:
                json.dump({"mode": mode, "items": trace["spec"]["items"]}, handle)
            return original(trace, script, started)
        soak._drive = drive
        return soak.run()

    def test_clean_run_end_to_end(self):
        trace = self.soak("clean")
        self.assertEqual(trace["end"]["reason"], "reached", trace["end"])
        self.assertIsNotNone(trace["followup"])
        self.assertTrue(trace["final"]["check_passed"])
        self.assertGreaterEqual(len(trace["saved_checkpoints"]), 1)
        self.assertEqual([answer["id"] for answer in trace["recall"]], ["early_fact", "rule", "count"])
        measures, _details = grade(trace, trace["spec"])
        self.assertEqual(harness_flags(measures), set())
        self.assertEqual(measures["recall_correct"], 3)
        self.assertEqual(measures["format_change_applied"], 1.0)
        self.assertGreater(trace["cost"], 0)

    def test_spend_cap_stops_the_run(self):
        trace = self.soak("expensive")
        self.assertEqual(trace["end"]["reason"], "cap_cost")
        self.assertGreater(trace["cost"], self.OPTIONS["max_run_cost"])
        measures, _details = grade(trace, trace["spec"])
        self.assertEqual(measures["health_breakage"], 0)
        self.assertEqual(measures["goal_continuation_breaks"], 0)
        # Stopped at the first turns, long before the goal could finish.
        self.assertLess(measures["goal_turns"], 4)

    def test_goal_that_stops_continuing_is_caught(self):
        trace = self.soak("stall")
        self.assertEqual(trace["end"]["reason"], "not_continuing")
        measures, _details = grade(trace, trace["spec"])
        self.assertEqual(measures["health_breakage"], 1)
        self.assertEqual(measures["goal_continuation_breaks"], 1)

    def test_history_lines_are_parsed_for_saved_checkpoints(self):
        line = json.dumps({"kind": "event", "envelope": {"event": {"kind": "context_checkpoint", "summary": "S"}}})
        self.assertEqual(checkpoint_summary(line), "S")
        self.assertIsNone(checkpoint_summary(json.dumps({"kind": "user", "text": "hi"})))
        self.assertIsNone(checkpoint_summary("{not json"))


class CliTests(unittest.TestCase):
    def test_report_and_trend_offline(self):
        from .__main__ import arms, main, parse_budget
        folder = tempfile.mkdtemp()
        try:
            trace_path = os.path.join(folder, "trace.json")
            with open(trace_path, "w", encoding="utf-8") as handle:
                json.dump(clean(), handle)
            history = os.path.join(folder, "history.jsonl")
            for i in range(3):
                trend.append(history, record(index=i))
            printed = io.StringIO()
            with contextlib.redirect_stdout(printed), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(main(["report", trace_path, "--history", history]), 0)
                self.assertEqual(main(["trend", "--history", history]), 0)
                self.assertEqual(main(["trend", "--history", os.path.join(folder, "none.jsonl")]), 1)
            self.assertIn("2 earlier run(s)", printed.getvalue())
        finally:
            shutil.rmtree(folder)
        self.assertEqual(parse_budget("auto"), "auto")
        self.assertEqual(parse_budget("24000"), 24000)
        self.assertEqual(set(arms([24000, "auto"], [1, 2])), {(24000, 1), ("auto", 1), (24000, 2), ("auto", 2)})
        self.assertEqual(arms([1, 2, 3], [5]), arms([1, 2, 3], [5]))


if __name__ == "__main__":
    unittest.main()
