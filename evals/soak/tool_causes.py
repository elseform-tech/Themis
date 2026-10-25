"""Classifies each failed tool call by cause, from its tool, error text and the run's approval
mode only (never from a model's opinion):

- `ours`: a tool crashed or returned an internal error, a valid patch was refused, a sandbox or
  approval refusal of an action the run's mode allows, or an error missing the fact needed to
  recover (for example "file not found" without the path);
- `model`: invalid arguments, a path that does not exist, an ambiguous or mismatched patch, a
  command that failed on its own;
- `unclassified`: none of the patterns matched. Listed first in the report for a person to sort;
  add a pattern here so the list stays short.

The patterns start from the error texts in `crates/core/src/tools.rs`, `tools/patch.rs` and the
runtime's `execute_call`.
"""

import re

# Custom-mode rules for the soak (decided 2026-10-25): every item must be read and written
# one at a time, so scripts, searches and other commands are denied. Only `python3 check.py`
# runs as a command. The default is deny, so nothing ever waits for an approval nobody answers.
ALLOWED_TOOLS = ("read_file", "list_dir", "write_file", "apply_patch", "recall", "set_goal", "git")
CHECK_COMMAND = ["python3", "check.py"]
APPROVAL = {
    "default": "deny",
    "rules": [{"tool": "shell", "command": CHECK_COMMAND, "action": "allow"}]
             + [{"tool": tool, "action": "allow"} for tool in ALLOWED_TOOLS],
}

# Checked first: these are our faults whatever the tool.
OURS = (
    ("internal", r"execution task failed|worker failed|failed to wait for|panicked|internal error"
                 r"|event queue is full|not available in this context"),
    # YOLO allows every action inside the project: an approval refusal there is ours.
    ("approval_refused", r"\bdenied\b.*\bapprov|\bapproval\b.*\b(denied|refused|required)|not allowed by the user"),
    ("sandbox_refused_project", r"sandbox blocked this command: .*(?:\bout/|\bsrc/|check\.py|INDEX\.md)"),
    ("patch_refused_valid", r"apply_patch: .*(?:could not commit|failed to write|rename)"),
)
MODEL = (
    ("batch_limit", r"^Not run: this batch|Tool arguments exceed"),
    ("bad_arguments", r"argument '[^']+' is invalid|invalid arguments|invalid (?:type|value|JSON)"
                      r"|missing field|unknown field|must not be empty"
                      # a JSONL field asked of a file that has none; a cwd that is no directory
                      r"|Selected JSONL field is missing|working directory '[^']*' is not a directory"),
    ("unknown_tool", r"unknown tool '"),
    ("missing_path", r"(?:no such file|not found|does not exist|cannot find).{0,80}['`\"/]"
                     r"|['`\"][^'`\"]+['`\"].{0,40}(?:no such file|not found|does not exist)"),
    ("outside_project", r"escapes the project root|outside the project"),
    ("patch_mismatch", r"patch does not apply|context mismatch|ambiguous|No files were changed"),
    ("command_failed", r"exit(?:ed)?(?: with)?(?: code| status)?\s*[:=]?\s*[1-9]|non-zero|command failed"
                       r"|failed to start '"),
    ("too_large", r"too large|exceeds? \d+ (?:bytes|characters)"),
)
# An error that names no path, file or quoted value cannot be acted on.
_MISSING_FACT = re.compile(r"^\W*(?:[a-z_]+: )?(?:file )?not found\W*$|^\W*(?:[a-z_]+: )?no such file or directory\W*$",
                           re.IGNORECASE)


def classify(tool, output, mode="yolo"):
    """(cause, code) for one failed call: cause in ours/model/unclassified, code a short name."""
    text = (output or "").strip()
    if _MISSING_FACT.match(text):
        return "ours", "error_without_fact"
    if mode == "custom" and re.search(r"^\W*(?:tool '[^']+' failed: )?approval denied for '", text):
        # The soak's rules allow these; anything else denied is the model reaching past them.
        allowed = tool in ALLOWED_TOOLS or (tool == "shell" and _runs_check(text))
        return ("ours", "approval_refused") if allowed else ("model", "denied_by_rule")
    for code, pattern in OURS:
        if code == "approval_refused" and mode != "yolo":
            continue
        if re.search(pattern, text, re.IGNORECASE | re.DOTALL):
            return "ours", code
    for code, pattern in MODEL:
        if re.search(pattern, text, re.IGNORECASE | re.MULTILINE):
            return "model", code
    if tool == "shell" and re.search(r"Traceback|Error|error:", text):
        return "model", "command_failed"
    return "unclassified", "unknown"


def _runs_check(text):
    """Whether a denied shell call was exactly the allowed check command."""
    return re.search(r'"command"\s*:\s*"python3"\s*,\s*"args"\s*:\s*\[\s*"check\.py"\s*\]', text) is not None


def sample(output, limit=120):
    """A short, single-line excerpt of an unclassified error for the report."""
    return " ".join((output or "").split())[:limit]
