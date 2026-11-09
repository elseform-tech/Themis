"""The integrations scenarios: the 2026-11-07 tasks, as requests that never name `themis` or a tool.

Each scenario has the person's `message`, the store it starts from (`seed`: plugins the person
saved with their own CLI before the chat, `{project}` replaced by the project folder), the
project's extra `files`, and a hidden `check` over the store afterwards (see `grade.py`)."""

NOTES_SERVER = '''import json, sys

TOOLS = [{"name": "list_notes", "description": "List notes", "inputSchema": {"type": "object"}}]

for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        result = {"protocolVersion": request["params"].get("protocolVersion", "2025-06-18"),
                  "capabilities": {"tools": {}}, "serverInfo": {"name": "notes", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": TOOLS}
    elif method == "tools/call":
        result = {"content": [{"type": "text", "text": "no notes"}]}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
'''

FILES = {"mcp/notes_server.py": NOTES_SERVER, "mcp/weather_server.py": NOTES_SERVER.replace("notes", "weather")}

CREATE_SKILL = {
    "name": "create-skill",
    "message": ("Create a skill called release-notes that drafts short release notes from the git log "
                "since the last tag. It should be available in all my chats."),
    "seed": [],
    "check": {"kind": "skill_saved", "id_contains": "release"},
}

ADD_MCP = {
    "name": "add-mcp",
    "message": ("Add an MCP server called notes for me. It is the script mcp/notes_server.py in this "
                "project, started with python3. Make sure it is set up correctly."),
    "seed": [],
    "check": {"kind": "mcp_saved", "name": "notes", "script": "mcp/notes_server.py"},
}

FIX_MCP = {
    "name": "fix-mcp",
    "message": "The notes MCP server in my tools plugin doesn't start. Please fix it.",
    "seed": [{"plugin": "tools", "mcp": {"notes": {"command": "python3",
                                                     "args": ["{project}/mcp/note_server.py"]}}}],
    "check": {"kind": "mcp_fixed", "plugin": "tools", "name": "notes", "script": "mcp/notes_server.py"},
}

ADD_HOOK = {
    "name": "add-hook",
    "message": ("Add a hook that stops any shell command containing `rm -rf` before it runs, "
                "so it applies in every chat."),
    "seed": [],
    "check": {"kind": "hook_saved", "contains": "rm -rf"},
}

DELETE_HOOK = {
    "name": "delete-hook",
    "message": "Remove the noisy hook from my guards plugin, but keep the other one.",
    "seed": [{"plugin": "guards", "hooks": [
        {"name": "noisy", "event": "AfterTool", "command": "echo noisy >> /tmp/noisy.log"},
        {"name": "keep", "event": "BeforeTool", "matcher": "shell", "command": "true"}]}],
    "check": {"kind": "hook_removed", "plugin": "guards", "gone": "noisy", "kept": "keep"},
}

DIAGNOSE_MCP = {
    "name": "diagnose-subagent",
    "message": ("Use a subagent to find out why the weather MCP server in my tools plugin fails. "
                "Don't change anything, just tell me what is wrong."),
    "seed": [{"plugin": "tools", "mcp": {"weather": {"command": "python3",
                                                       "args": ["{project}/mcp/wether_server.py"]}}}],
    "check": {"kind": "diagnosed", "plugin": "tools", "name": "weather", "cause": "wether_server.py"},
}

PROJECT_SKILL = {
    "name": "project-skill",
    "message": ("Add a skill called release-checklist for this project that lists the steps to check "
                "before a release. My team should get it too through the repository."),
    "seed": [],
    "check": {"kind": "project_skill", "id_contains": "release"},
}

ALL = {s["name"]: s for s in (CREATE_SKILL, ADD_MCP, FIX_MCP, ADD_HOOK, DELETE_HOOK, DIAGNOSE_MCP,
                              PROJECT_SKILL)}


def load(name):
    if name not in ALL:
        raise ValueError(f"unknown scenario {name!r}; choose from {', '.join(sorted(ALL))}")
    return ALL[name]
