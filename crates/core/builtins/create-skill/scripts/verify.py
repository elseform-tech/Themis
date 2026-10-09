#!/usr/bin/env python3
"""Validate a Themis JSON skill draft or a SKILL.md directory. Stdlib only."""
import json
import re
import sys
from pathlib import Path

TOOLS = {"list_dir", "read_file", "write_file", "copy_file", "move_file", "delete_file", "create_dir", "search_file", "shell", "apply_patch", "git"}

def verify(path):
    path = Path(path)
    if path.is_dir():
        for entry in path.rglob("*"):
            if entry.is_symlink():
                raise ValueError("skill directory must not contain symlinks")
        path = path / "SKILL.md"
    if path.stat().st_size > 2 * 1024 * 1024:
        raise ValueError("draft exceeds 2 MiB")
    text = path.read_text(encoding="utf-8")
    if path.name == "SKILL.md":
        match = re.match(r"\A---\r?\n(.*?)\r?\n---\r?\n(.*)\Z", text, re.S)
        if not match:
            raise ValueError("SKILL.md requires closed YAML frontmatter")
        metadata = {}
        for line in match[1].splitlines():
            if ":" in line and not line.startswith(" "):
                key, value = line.split(":", 1)
                metadata[key] = value.strip().strip("\"'")
        skill = {"id": metadata.get("name"), "name": metadata.get("name"), "description": metadata.get("description"), "instructions": match[2]}
        if not skill["description"]:
            raise ValueError("description is required in SKILL.md")
    else:
        skill = json.loads(text)
    if not isinstance(skill, dict):
        raise ValueError("skill must be an object")
    if not isinstance(skill.get("description"), str):
        raise ValueError("description must be text")
    if path.name != "SKILL.md" and ("scripts" not in skill or ("allowedTools" not in skill and "allowed_tools" not in skill)):
        raise ValueError("JSON draft requires scripts and allowedTools lists")
    for key in ("id", "name", "instructions"):
        if not isinstance(skill.get(key), str) or not skill[key].strip():
            raise ValueError(f"{key} must be nonempty text")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,99}", skill["id"]) or "--" in skill["id"]:
        raise ValueError("id must be a safe single name without --")
    if len(skill["instructions"].encode()) > 256 * 1024:
        raise ValueError("instructions exceed 256 KiB")
    tools = skill.get("allowedTools", skill.get("allowed_tools", []))
    if not isinstance(tools, list) or any(not isinstance(t, str) or t not in TOOLS for t in tools):
        raise ValueError("allowedTools contains an unknown tool")
    scripts = skill.get("scripts", [])
    if not isinstance(scripts, list):
        raise ValueError("scripts must be a list")
    names = set()
    total = 0
    for script in scripts:
        if not isinstance(script, dict):
            raise ValueError("script must be an object")
        name = script.get("name", "")
        if not isinstance(name, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", name) or name in names:
            raise ValueError("unsafe or duplicate script name")
        names.add(name)
        if not isinstance(script.get("content"), str):
            raise ValueError("script content must be text")
        total += len(script["content"].encode())
    if total > 65536:
        raise ValueError("scripts exceed 64 KiB")
    return skill["id"]

if __name__ == "__main__":
    try:
        if len(sys.argv) != 2:
            raise ValueError("usage: verify.py <skill.json|SKILL.md|skill-directory>")
        print(f"Valid skill: {verify(sys.argv[1])}")
    except (ValueError, OSError, TypeError) as error:
        print(f"Invalid skill: {error}", file=sys.stderr)
        sys.exit(1)
