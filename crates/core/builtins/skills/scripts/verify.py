#!/usr/bin/env python3
"""Validate a Themis JSON skill draft or a SKILL.md directory. Stdlib only. Reports every problem."""
import json
import os
import re
import sys
from pathlib import Path

# The tool names a skill may list, as the store knows them (crates/core/src/skills.rs).
TOOLS = {"read_file", "write_file", "shell", "background_task", "recall", "apply_patch", "fetch_url", "web_search", "browser"}
if os.name == "nt":
    TOOLS |= {"list_dir", "search_file"}
FIELDS = {"id", "name", "description", "instructions", "allowedTools", "allowed_tools", "scripts"}


def load(path):
    if path.is_dir():
        for entry in path.rglob("*"):
            if entry.is_symlink():
                raise ValueError("skill directory must not contain symlinks")
        path = path / "SKILL.md"
    if path.stat().st_size > 2 * 1024 * 1024:
        raise ValueError("draft exceeds 2 MiB")
    text = path.read_text(encoding="utf-8")
    if path.name != "SKILL.md":
        return json.loads(text), []
    match = re.match(r"\A---\r?\n(.*?)\r?\n---\r?\n(.*)\Z", text, re.S)
    if not match:
        raise ValueError("SKILL.md requires closed YAML frontmatter")
    metadata = {}
    for line in match[1].splitlines():
        if ":" in line and not line.startswith(" "):
            key, value = line.split(":", 1)
            metadata[key] = value.strip().strip("\"'")
    skill = {"id": metadata.get("name"), "name": metadata.get("name"), "description": metadata.get("description"), "instructions": match[2]}
    return skill, [] if skill["description"] else ["description is required in SKILL.md"]


def problems(skill):
    if not isinstance(skill, dict):
        return ["skill must be an object"]
    found = [f"{key}: unknown field" for key in skill if key not in FIELDS]
    if not isinstance(skill.get("description", ""), str):
        found.append("description must be text")
    for key in ("id", "name", "instructions"):
        if not isinstance(skill.get(key), str) or not skill[key].strip():
            found.append(f"{key} must be nonempty text")
    if isinstance(skill.get("id"), str) and (not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,99}", skill["id"]) or "--" in skill["id"]):
        found.append("id must be a safe single name without --")
    if isinstance(skill.get("instructions"), str) and len(skill["instructions"].encode()) > 256 * 1024:
        found.append("instructions exceed 256 KiB")
    tools = skill.get("allowedTools", skill.get("allowed_tools", []))
    if not isinstance(tools, list) or any(not isinstance(t, str) or t not in TOOLS for t in tools):
        found.append("allowedTools contains an unknown tool; known: " + ", ".join(sorted(TOOLS)))
    found.extend(script_problems(skill.get("scripts", [])))
    return found


def script_problems(scripts):
    if not isinstance(scripts, list):
        return ["scripts must be a list"]
    found = []
    names = set()
    total = 0
    for script in scripts:
        if not isinstance(script, dict):
            found.append("script must be an object")
            continue
        name = script.get("name", "")
        if not isinstance(name, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", name) or name in names:
            found.append(f"unsafe or duplicate script name {name!r}")
        names.add(name if isinstance(name, str) else "")
        if not isinstance(script.get("content"), str):
            found.append(f"script {name!r} content must be text")
        else:
            total += len(script["content"].encode())
    if total > 65536:
        found.append("scripts exceed 64 KiB")
    return found


def verify(path):
    skill, found = load(Path(path))
    found += problems(skill)
    if found:
        raise ValueError("; ".join(found))
    return skill["id"]


if __name__ == "__main__":
    try:
        if len(sys.argv) != 2:
            raise ValueError("usage: verify.py <skill.json|SKILL.md|skill-directory>")
        print(f"Valid skill: {verify(sys.argv[1])}")
    except (ValueError, OSError, TypeError) as error:
        print(f"Invalid skill: {error}", file=sys.stderr)
        sys.exit(1)
