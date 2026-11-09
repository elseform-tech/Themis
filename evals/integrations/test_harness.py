"""Offline checks of the integrations eval's grading (no app, no model)."""

import os
import tempfile
import unittest

from evals.integrations import grade, scenarios


def store(*plugins):
    return [{"scope": scope, "source": None, "spec": spec} for scope, spec in plugins]


def item(kind, **event):
    return {"t": 0, "run_id": "r", "event": {"kind": kind, **event}}


class Checks(unittest.TestCase):
    def setUp(self):
        self.project = tempfile.mkdtemp()
        os.makedirs(os.path.join(self.project, "mcp"))
        open(os.path.join(self.project, "mcp", "notes_server.py"), "w").close()

    def test_a_fixed_server_must_start_the_real_script(self):
        scenario = scenarios.load("fix-mcp")
        broken = store(("global", {"name": "tools", "mcp": {"notes": {"command": "python3", "args": [
            os.path.join(self.project, "mcp/note_server.py")]}}}))
        fixed = store(("global", {"name": "tools", "mcp": {"notes": {"command": "python3", "args": [
            "mcp/notes_server.py"], "cwd": "."}}}))
        self.assertEqual(grade.hidden_check(scenario, broken, self.project)["met"], 0)
        self.assertEqual(grade.hidden_check(scenario, fixed, self.project)["met"], 1)

    def test_a_skill_in_the_project_is_not_for_every_chat(self):
        scenario = scenarios.load("create-skill")
        skill = {"id": "release-notes", "instructions": "Draft notes."}
        self.assertEqual(grade.hidden_check(scenario, store(("local", {"name": "p", "skills": [skill]})),
                                            self.project)["met"], 0)
        self.assertEqual(grade.hidden_check(scenario, store(("global", {"name": "p", "skills": [skill]})),
                                            self.project)["met"], 1)

    def test_deleting_one_hook_keeps_the_other(self):
        scenario = scenarios.load("delete-hook")
        both = store(("global", {"name": "guards", "hooks": [{"name": "noisy"}, {"name": "keep"}]}))
        none = store(("global", {"name": "guards", "hooks": []}))
        right = store(("global", {"name": "guards", "hooks": [{"name": "keep"}]}))
        self.assertEqual([grade.hidden_check(scenario, s, self.project)["met"] for s in (both, none, right)],
                         [0, 0, 1])

    def test_a_diagnosis_names_the_cause_and_changes_nothing(self):
        scenario = scenarios.load("diagnose-subagent")
        before = store(("global", {"name": "tools", "mcp": {"weather": {"command": "python3", "args": ["x/wether_server.py"]}}}))
        changed = store(("global", {"name": "tools", "mcp": {"weather": {"command": "python3", "args": ["y"]}}}))
        reply = "The script path says wether_server.py, which does not exist."
        self.assertEqual(grade.hidden_check(scenario, before, self.project, reply, before)["met"], 1)
        self.assertEqual(grade.hidden_check(scenario, changed, self.project, reply, before)["met"], 0)
        self.assertEqual(grade.hidden_check(scenario, before, self.project, "It times out.", before)["met"], 0)


class Measures(unittest.TestCase):
    def test_failed_themis_calls_old_tools_and_false_claims_are_counted(self):
        trace = {"store": store(("global", {"name": "guards", "hooks": [{"name": "noisy"}, {"name": "keep"}]})),
                 "actions": [], "approvals": [], "events": [
                     item("started"),
                     item("tool_started", tool="shell", summary="themis hook delete guards noisy"),
                     item("tool_finished", tool="shell", ok=True, output='{"exit_code": 1, "stderr": "Unknown"}'),
                     item("tool_started", tool="manage_integrations", summary="{}"),
                     item("tool_finished", tool="manage_integrations", ok=False, output="bad"),
                     item("tool_started", tool="shell", summary='call `shell` with {"command":"themis","args":["plugin","list"]}'),
                     item("tool_finished", tool="shell", ok=True, output='{"exit_code": 0}'),
                     item("assistant_text", text="I removed the noisy hook."),
                 ]}
        measures = grade.evaluate(trace, scenarios.load("delete-hook"), "/nowhere")
        self.assertEqual((measures["met"], measures["false_claim"]), (0, 1))
        self.assertEqual((measures["tool_calls"], measures["failed_calls"]), (3, 2))
        self.assertEqual((measures["old_tool_calls"], measures["themis_calls"], measures["failed_themis_calls"]),
                         (1, 2, 1))

    def test_a_reply_saying_it_could_not_is_no_false_claim(self):
        trace = {"store": [], "actions": [], "approvals": [], "events": [
            item("started"), item("assistant_text", text="I could not remove it: the plugin was not found.")]}
        self.assertEqual(grade.evaluate(trace, scenarios.load("delete-hook"), "/nowhere")["false_claim"], 0)


if __name__ == "__main__":
    unittest.main()
