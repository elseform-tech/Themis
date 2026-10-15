"""How each tool call went: success, or the reason it was wasted."""

import json
import re

REASONS = [
    ("refused", re.compile(r"approval denied|not approved|refused", re.I)),
    ("bad_arguments", re.compile(r"invalid (type|value|argument)|missing field|unknown field|expected (a|an|one)|"
                                 r"failed to parse|must (be|not)|requires|unknown (kind|tool)|is not allowed", re.I)),
    ("patch_format", re.compile(r"hunk|patch|context (line|mismatch)|@@", re.I)),
    ("wrong_path", re.compile(r"absolute paths|no such file|not found|does not exist|outside|escapes", re.I)),
]


def _json(text):
    try:
        value = json.loads(text)
    except (TypeError, ValueError):
        return None
    return value if isinstance(value, dict) else None


def outcome(call):
    """None when the call did what it was asked, otherwise a short reason it was wasted."""
    output = call.get("output", "")
    if call.get("decision") == "deny":
        return "refused"
    if not call.get("ok"):
        for reason, pattern in REASONS:
            if pattern.search(output):
                return reason
        return "other_error"
    parsed = _json(output)
    if call["tool"] == "shell" and parsed and parsed.get("exit_code") not in (None, 0):
        return "command_failed"
    if call["tool"] == "recall" and parsed and parsed.get("total_matches") == 0:
        return "empty_result"
    return None


def call_rows(report):
    """Every tool call with its outcome and whether it was the first call of that tool."""
    seen = set()
    rows = []
    for call in report.get("calls", []):
        tool = call["tool"]
        reason = outcome(call)
        rows.append({"tool": tool, "first": tool not in seen, "reason": reason,
                     "summary": call.get("summary", "")[:200],
                     "output": call.get("output", "")[:300] if reason else ""})
        seen.add(tool)
    return rows
