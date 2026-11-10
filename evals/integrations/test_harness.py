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

    def test_a_project_skill_is_a_shared_skill_folder_not_a_stored_skill(self):
        scenario = scenarios.load("project-skill")
        folder = os.path.join(self.project, ".agents", "skills", "release-checklist")
        self.assertEqual(grade.hidden_check(scenario, store(), self.project)["met"], 0)
        os.makedirs(folder)
        with open(os.path.join(folder, "SKILL.md"), "w") as handle:
            handle.write("---\nname: release-checklist\ndescription: Steps before a release\n---\n1. Run the tests.\n")
        self.assertEqual(grade.hidden_check(scenario, store(), self.project)["met"], 1)
        stored = store(("local", {"name": "personal", "skills": [{"id": "release-checklist"}]}))
        self.assertEqual(grade.hidden_check(scenario, stored, self.project)["met"], 0)

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

    def test_a_loaded_guide_and_a_doctor_run_are_counted(self):
        trace = {"store": [], "actions": [], "approvals": [], "events": [
            item("started"),
            item("tool_started", tool="Skill", summary='call `Skill` with {"skill":"mcp"}'),
            item("tool_finished", tool="Skill", ok=True, output="..."),
            item("tool_started", tool="shell",
                 summary='call `shell` with {"command":"themis","args":["doctor","--json","--integrations"]}'),
            item("tool_finished", tool="shell", ok=True, output='{"exit_code": 0}'),
        ]}
        measures = grade.evaluate(trace, scenarios.load("add-mcp"), "/nowhere")
        self.assertEqual((measures["guide_loaded"], measures["doctor_run"]), (1, 1))
        quiet = {"store": [], "actions": [], "approvals": [], "events": [
            item("started"),
            item("tool_started", tool="Skill", summary='call `Skill` with {"skill":"init"}'),
            item("tool_started", tool="shell", summary='call `shell` with {"command":"themis","args":["doctor"]}'),
            item("tool_started", tool="read_file", summary="read .themis/skills/hooks/SKILL.md"),
        ]}
        measures = grade.evaluate(quiet, scenarios.load("add-mcp"), "/nowhere")
        self.assertEqual((measures["guide_loaded"], measures["doctor_run"]), (1, 0))

    def test_a_reply_naming_the_personal_plugin_is_counted(self):
        for reply, named in (("Switch notes on in Integrations (under the `personal` plugin).", 1),
                             ("Saved in your personal plugin.", 1),
                             ("Switch notes on in Integrations; it is listed under Yours.", 0),
                             ("I ran themis mcp add personal notes.", 0)):
            self.assertEqual(int(bool(grade.HOLDER.search(reply))), named, reply)

    def test_files_left_in_the_project_are_counted(self):
        trace = {"events": [], "store": {}, "project_changes": [
            "scratchpad/glob-greet.json", ".mcp.json", ".themis/context/x.jsonl",
            ".agents/skills/release-checklist/SKILL.md"]}
        self.assertEqual(grade.evaluate(trace, scenarios.load("delete-hook"), "/nowhere")["stray_files"], 2)

    def test_a_reply_saying_it_could_not_is_no_false_claim(self):
        trace = {"store": [], "actions": [], "approvals": [], "events": [
            item("started"), item("assistant_text", text="I could not remove it: the plugin was not found.")]}
        self.assertEqual(grade.evaluate(trace, scenarios.load("delete-hook"), "/nowhere")["false_claim"], 0)


if __name__ == "__main__":
    unittest.main()
