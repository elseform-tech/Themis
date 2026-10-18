"""Measures of how well notes carry work across checkpoints, from a runner report (calls,
checkpoints with their notes and place in `calls`, the notes after every notes call).

- stale lines: task lines a checkpoint shows open that the first notes update after it marks
  done before any other work, so they were finished but not recorded when it was taken;
- redone writes: after a checkpoint, a write_file call identical to one made before it;
- rewrites: after a checkpoint, a write_file to a path already written before it;
- notes age: tool calls since the last notes update, at each checkpoint;
- wasted repeats: a call identical to an earlier one with only reads between them, so
  nothing it could see had changed (a test rerun after an edit is not counted);
- refreshes: notes-only requests the runtime sent before checkpoints.
"""

import re

OPEN = ("pending", "in_progress")
# Calls that change nothing a later identical call could see.
READS = ("read_file", "list_dir", "search_file", "recall", "notes")
# Saved summaries are cut here without a marker, so two cut ones may differ past it.
SUMMARY_LIMIT = 200


def _lines(notes):
    """Notes lines from a checkpoint (a list) or a saved notes file ({"lines": [...]})."""
    if isinstance(notes, dict):
        notes = notes.get("lines")
    return notes if isinstance(notes, list) else []


def _path(summary):
    found = re.search(r'"file_path":"((?:[^"\\]|\\.)*)"', summary or "")
    return found.group(1) if found else None


def stale_lines(row):
    """Open lines per checkpoint that were already done (see module doc), summed over checkpoints."""
    calls, log = row.get("calls") or [], row.get("notes_log") or []
    notes_positions = [index for index, call in enumerate(calls) if call.get("tool") == "notes"]
    total = 0
    for point in row.get("checkpoints") or []:
        first = point.get("notes_calls_before", 0)
        if first >= len(log) or first >= len(notes_positions):
            continue
        # Only a notes update that comes before any other work shows what was already done.
        between = calls[point.get("calls_before", 0):notes_positions[first]]
        if any(call.get("tool") != "notes" for call in between):
            continue
        after = {line.get("key"): line.get("status") for line in _lines(log[first])}
        total += sum(1 for line in _lines(point.get("notes"))
                     if line.get("status") in OPEN and after.get(line.get("key")) == "done")
    return total


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


def notes_age(row):
    """Mean tool calls since the last notes update at each checkpoint (None without any)."""
    calls, ages = row.get("calls") or [], []
    for point in row.get("checkpoints") or []:
        position = point.get("calls_before", 0)
        last = max((index for index, call in enumerate(calls[:position]) if call.get("tool") == "notes"),
                   default=None)
        ages.append(position if last is None else position - last - 1)
    return sum(ages) / len(ages) if ages else None


def wasted_repeats(row):
    """Calls that repeat an earlier identical call with only reads between them. Notes
    updates are not counted, and neither are summaries cut at the limit."""
    calls, wasted = row.get("calls") or [], 0
    for index, call in enumerate(calls):
        summary = call.get("summary") or ""
        if call.get("tool") == "notes" or len(summary) >= SUMMARY_LIMIT:
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
    points = row.get("checkpoints") or []
    if any("calls_before" not in point for point in points) or "notes_refreshes" not in row:
        return None
    redone, rewrites = writes_after_checkpoints(row)
    return {"stale_lines": stale_lines(row), "redone_writes": redone, "rewrites_after_checkpoint": rewrites,
            "notes_age": notes_age(row), "wasted_repeats": wasted_repeats(row), "notes_refreshes": row.get("notes_refreshes", 0)}
