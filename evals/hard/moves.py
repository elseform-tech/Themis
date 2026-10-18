"""Measures of redone work and wasted moves, from a runner report (calls, and checkpoints
with their place in `calls`).

- redone writes: after a checkpoint, a write_file call identical to one made before it;
- rewrites: after a checkpoint, a write_file to a path already written before it;
- re-reads after checkpoints: the first read of a file after a checkpoint when the run had
  already read or written it before that checkpoint (what a checkpoint failed to carry);
- wasted repeats: a call identical to an earlier one with only reads between them, so
  nothing it could see had changed (a test rerun after an edit is not counted).
"""

import re

# Calls that change nothing a later identical call could see.
READS = ("read_file", "list_dir", "search_file", "recall")
# Saved summaries are cut here without a marker, so two cut ones may differ past it.
SUMMARY_LIMIT = 200
MEASURES = ("redone_writes", "rewrites_after_checkpoint", "rereads_after_checkpoint", "wasted_repeats")


def _path(summary):
    found = re.search(r'"file_path":"((?:[^"\\]|\\.)*)"', summary or "")
    return found.group(1) if found else None


def writes_after_checkpoints(row):
    """(redone, rewrites): write_file calls after a checkpoint that repeat an earlier call
    exactly, or target a path written before it."""
    calls = row.get("calls") or []
    redone = rewrites = 0
    for index, call in enumerate(calls):
        if call.get("tool") != "write_file" or not _path(call.get("summary")):
            continue
        before = max((point.get("calls_before", 0) for point in row.get("checkpoints") or []
                      if point.get("calls_before", 0) <= index), default=None)
        if before is None:
            continue
        earlier = [c for c in calls[:before] if c.get("tool") == "write_file"]
        if any(c.get("summary") == call.get("summary") for c in earlier):
            redone += 1
        elif any(_path(c.get("summary")) == _path(call.get("summary")) for c in earlier):
            rewrites += 1
    return redone, rewrites


def rereads_after_checkpoints(row):
    """First reads of a file after each checkpoint that the run had read or written before it."""
    calls, rereads = row.get("calls") or [], 0
    points = sorted(point.get("calls_before", 0) for point in row.get("checkpoints") or [])
    for start, end in zip(points, points[1:] + [len(calls)]):
        seen = {_path(c.get("summary")) for c in calls[:start] if c.get("tool") in ("read_file", "write_file")}
        again = set()
        for call in calls[start:end]:
            path = _path(call.get("summary"))
            if call.get("tool") == "read_file" and path in seen and path not in again:
                rereads += 1
            if path:
                again.add(path)
    return rereads


def wasted_repeats(row):
    """Calls that repeat an earlier identical call with only reads between them; summaries
    cut at the limit are not compared."""
    calls, wasted = row.get("calls") or [], 0
    for index, call in enumerate(calls):
        summary = call.get("summary") or ""
        if len(summary) >= SUMMARY_LIMIT:
            continue
        for earlier in reversed(calls[:index]):
            if earlier.get("tool") == call.get("tool") and earlier.get("summary") == summary:
                wasted += 1
                break
            if earlier.get("tool") not in READS:
                break
    return wasted


def measure(row):
    """The measures for one run, or None when its runner did not record checkpoint places."""
    if any("calls_before" not in point for point in row.get("checkpoints") or []):
        return None
    redone, rewrites = writes_after_checkpoints(row)
    return {"redone_writes": redone, "rewrites_after_checkpoint": rewrites,
            "rereads_after_checkpoint": rereads_after_checkpoints(row), "wasted_repeats": wasted_repeats(row)}
