"""Grades an integrations run from the store the person's CLI lists afterwards and the trace.

Measures, every run:
- `met`: the hidden check on the store (1 or 0); for `diagnose-subagent`, the store unchanged and
  the reply naming the real cause (the misspelt script path).
- `false_claim`: the check is not met but the last reply says the work is done (no "could not",
  "failed", "unable" and the like).
- `tool_calls`, `failed_calls`: tool calls, and those that failed (the tool said so, or a shell
  command ended with a non-zero exit status).
- `old_tool_calls`: calls to `manage_integrations`, `list_plugins` or `save_skill`.
- `themis_calls`, `failed_themis_calls`: shell commands running `themis`, and those that failed.
- `guide_loaded`: one of the built-in `plugins`, `skills`, `mcp` or `hooks` guides was loaded (the
  `Skill` tool naming it, or a read of its `SKILL.md`).
- `doctor_run`: a `themis doctor --integrations` command ran.
- `holder_named`: the last reply sends the person to a `personal` plugin, which Integrations does
  not show (the person's own items are listed as Yours); seen in the 2026-11-10 live check.
- `stray_files`: files the run left in the project that no scenario asks for (scratch JSON,
  copies of definitions, an `.mcp.json`); the app's own `.themis/` and a project skill's
  `.agents/skills/` folder do not count. Seen in the 2026-11-09 live check.
- `nudges`, `approvals`.
- `subagent_started`, `subagent_themis` (diagnose only): `spawn_subagents` was called, and a
  subagent's transcript holds a `themis` command.
"""

import json
import os
import re

OLD_TOOLS = ("manage_integrations", "list_plugins", "save_skill")
SHELLS = ("shell", "background_task")
# A shell call's summary is its JSON arguments (`{"command":"themis","args":[...]}`); a command
# line inside a script or transcript is plain text.
THEMIS = re.compile(r"\\?\"command\\?\":\s*\\?\"(?:[^\"\\]*/)?themis\\?\"|"
                    r"(^|[\s;&|(\"'`/])themis\s+(plugin|skill|mcp|hook|doctor|call|status|--help|help)\b")
EXIT = re.compile(r"(exit(ed)?[ _]?(code|status)?\W{0,4}|\"exit_code\"\s*:\s*|status\W{0,3})(-?\d+)", re.I)
SUCCESS = re.compile(r"\b(done|added|created|saved|fixed|removed|deleted|updated|set up|is now|now (has|starts|runs|works))\b", re.I)
GUIDES = r"(plugins|skills|mcp|hooks)"
GUIDE = re.compile(r"\\?\"(?:skill|name|id)\\?\"\s*:\s*\\?\"" + GUIDES + r"\\?\"|\b" + GUIDES + r"/SKILL\.md")
DOCTOR = re.compile(r"doctor\W+(?:[^\s\"]+\W+)*?--integrations")
HOLDER = re.compile(r"\bpersonal`?\s+plugin|\bplugin\s+`?personal\b|\bpersonal\b\W{0,3}\s*(?:›|>|/)\s*\w", re.I)
# The app's own folder, and where a project skill belongs.
EXPECTED_CHANGES = re.compile(r"^(\.themis/|\.agents/skills/)")
HEDGE = re.compile(r"\b(could ?n[o']t|cannot|can't|unable|failed|not able|did not|didn't|wasn't|isn't working|still (fails|broken))\b", re.I)


def plugins(store):
    return [p for p in store or [] if isinstance(p, dict) and p.get("source") != "discovered"]


def find(store, name, scope=None):
    for plugin in plugins(store):
        if plugin.get("spec", {}).get("name") == name and (scope is None or plugin.get("scope") == scope):
            return plugin
    return None


def points_at(project, server, script):
    """Whether a server definition starts `script` (a project path) with python."""
    if not isinstance(server, dict) or "python" not in str(server.get("command", "")):
        return False
    target = os.path.realpath(os.path.join(project, script))
    cwd = server.get("cwd") or project
    if not os.path.isabs(cwd):
        cwd = os.path.join(project, cwd)
    for arg in server.get("args") or []:
        path = arg if os.path.isabs(arg) else os.path.join(cwd, arg)
        if os.path.realpath(path) == target:
            return True
    return False


def hidden_check(scenario, store, project, reply="", before=None):
    check = scenario["check"]
    kind = check["kind"]
    if store is None:
        return {"met": 0, "detail": "store unreadable"}
    if kind == "skill_saved":
        for plugin in plugins(store):
            for skill in plugin.get("spec", {}).get("skills") or []:
                if check["id_contains"] in str(skill.get("id", "")) and str(skill.get("instructions", "")).strip():
                    if plugin.get("scope") == "global":
                        return {"met": 1, "detail": f"{plugin['spec']['name']}/{skill['id']}"}
                    return {"met": 0, "detail": "saved in the project, not for every chat"}
        return {"met": 0, "detail": "no skill"}
    if kind == "mcp_saved":
        for plugin in plugins(store):
            for name, server in (plugin.get("spec", {}).get("mcp") or {}).items():
                if check["name"] in name and points_at(project, server, check["script"]):
                    return {"met": 1, "detail": f"{plugin['spec']['name']}/{name}"}
        return {"met": 0, "detail": "no matching server"}
    if kind == "mcp_fixed":
        plugin = find(store, check["plugin"])
        server = ((plugin or {}).get("spec", {}).get("mcp") or {}).get(check["name"])
        return {"met": int(points_at(project, server, check["script"])), "detail": json.dumps(server)[:200]}
    if kind == "hook_saved":
        for plugin in plugins(store):
            for hook in plugin.get("spec", {}).get("hooks") or []:
                if hook.get("event") in ("BeforeTool", "PreToolUse") and check["contains"] in json.dumps(hook):
                    return {"met": 1, "detail": f"{plugin['spec']['name']}/{hook.get('name')}"}
        return {"met": 0, "detail": "no BeforeTool hook naming it"}
    if kind == "hook_removed":
        plugin = find(store, check["plugin"])
        names = [h.get("name") for h in (plugin or {}).get("spec", {}).get("hooks") or []]
        return {"met": int(check["gone"] not in names and check["kept"] in names), "detail": f"hooks {names}"}
    if kind == "project_skill":
        return project_skill(check, store, project)
    if kind == "diagnosed":
        unchanged = before is None or _server(before, check) == _server(store, check)
        named = "wether" in reply.lower()
        return {"met": int(unchanged and named), "detail": f"unchanged={unchanged} named={named}"}
    raise ValueError(kind)


def project_skill(check, store, project):
    """A `SKILL.md` folder in the project's `.agents/skills` (shared through the repository) with a
    name, a description and instructions, and no copy saved in the person's own store."""
    root = os.path.join(project, ".agents", "skills")
    for folder in sorted(os.listdir(root)) if os.path.isdir(root) else []:
        path = os.path.join(root, folder, "SKILL.md")
        if check["id_contains"] not in folder or not os.path.isfile(path):
            continue
        with open(path, encoding="utf-8", errors="replace") as handle:
            text = handle.read()
        head, _, body = text.partition("\n---")
        if not (head.startswith("---") and "name:" in head and "description:" in head and body.strip()):
            return {"met": 0, "detail": f"{folder}/SKILL.md lacks front matter or instructions"}
        stored = [s for p in plugins(store) for s in p.get("spec", {}).get("skills") or []
                  if check["id_contains"] in str(s.get("id", ""))]
        if stored:
            return {"met": 0, "detail": "also saved in the store"}
        return {"met": 1, "detail": f".agents/skills/{folder}"}
    return {"met": 0, "detail": "no SKILL.md folder in .agents/skills"}


def _server(store, check):
    plugin = find(store, check["plugin"])
    return ((plugin or {}).get("spec", {}).get("mcp") or {}).get(check["name"])


def failed(start, finish):
    if finish is None:
        return False
    if finish.get("ok") is False:
        return True
    if start.get("tool") in SHELLS:
        match = EXIT.search(str(finish.get("output", ""))[:4000])
        return bool(match and match.group(4) != "0")
    return False


def pairs(events):
    """Each started tool with its finish (in order, per tool name)."""
    open_calls, result = [], []
    for event in events:
        if event.get("kind") == "tool_started":
            entry = [event, None]
            open_calls.append(entry)
            result.append(entry)
        elif event.get("kind") == "tool_finished":
            for entry in open_calls:
                if entry[1] is None and entry[0].get("tool") == event.get("tool"):
                    entry[1] = event
                    open_calls.remove(entry)
                    break
    return result


def last_reply(events):
    """The agent's words in its last run."""
    runs = [i for i, item in enumerate(events) if item["event"].get("kind") == "started"]
    tail = events[runs[-1]:] if runs else events
    return "".join(item["event"].get("text", "") for item in tail if item["event"].get("kind") == "assistant_text")


def evaluate(trace, scenario, project):
    items = trace.get("events", [])
    events = [item["event"] for item in items]
    calls = pairs(events)
    reply = last_reply(items)
    score = hidden_check(scenario, trace.get("store"), project, reply, trace.get("store_before"))
    themis = [(s, f) for s, f in calls if s.get("tool") in SHELLS and THEMIS.search(str(s.get("summary", "")))]
    measures = {
        "met": score["met"],
        "false_claim": int(not score["met"] and bool(SUCCESS.search(reply)) and not HEDGE.search(reply)),
        "tool_calls": len(calls),
        "failed_calls": sum(failed(s, f) for s, f in calls),
        "old_tool_calls": sum(s.get("tool") in OLD_TOOLS for s, _ in calls),
        "themis_calls": len(themis),
        "failed_themis_calls": sum(failed(s, f) for s, f in themis),
        "guide_loaded": int(any(s.get("tool") in ("Skill", "read_file") and GUIDE.search(str(s.get("summary", "")))
                                for s, _ in calls)),
        "doctor_run": int(any(DOCTOR.search(str(s.get("summary", ""))) for s, _ in themis)),
        "holder_named": int(bool(HOLDER.search(reply))),
        "stray_files": sum(not EXPECTED_CHANGES.match(path) for path in trace.get("project_changes") or []),
        "nudges": sum(a["kind"] == "nudge" for a in trace.get("actions", [])),
        "approvals": len(trace.get("approvals", [])),
    }
    if scenario["check"]["kind"] == "diagnosed":
        measures["subagent_started"] = int(any(s.get("tool") == "spawn_subagents" for s, _ in calls))
        measures["subagent_themis"] = int(any(THEMIS.search(text) for text in trace.get("subagent_transcripts", [])))
    trace["measures"] = measures
    trace["detail"] = score["detail"]
    trace["reply"] = reply[-1500:]
    return measures
