"""Grades mid-turn runs: how often the agent re-reads what it already read once a checkpoint
replaced it.

A read is `read_file` of one (path, offset, limit, find) slice. Reads of the run's own saved
tool outputs under `.themis/context/` are counted apart as `evidence_reads`: recovering them is
what the checkpoint asks for. It counts as a re-read after
compaction when the same slice was read before the latest checkpoint and the file has not
been written since that read; a re-read with no checkpoint in between is counted separately
as background repetition. Writes are `write_file` and `apply_patch` (every `+++ b/` path).
"""

import json
import re
import statistics

PATCH_PATH = re.compile(r"\+\+\+ b/([^\s\\\"]+)")


def _arguments(summary):
    """The JSON arguments of a `call `tool` with {...}` summary, or {} when not parseable."""
    start = summary.find("{")
    if start < 0:
        return {}
    try:
        return json.loads(summary[start:])
    except ValueError:
        # Long summaries are cut short; keep the leading fields that survived.
        fields = {}
        for key, value in re.findall(r'"(file_path|find|patch)":"((?:[^"\\]|\\.)*)', summary):
            fields.setdefault(key, value)
        for key, value in re.findall(r'"(offset|limit)":(\d+)', summary):
            fields.setdefault(key, int(value))
        return fields


def grade_run(report):
    steps = report.get("steps") or []
    checkpoint = 0          # checkpoints seen so far
    read_at = {}            # slice -> checkpoint count at its last read
    written_at = {}         # path -> step index of its last write
    read_step = {}          # slice -> step index of its last read
    reads = rereads = repeats = reads_after = evidence = 0
    for index, step in enumerate(steps):
        if step["kind"] == "checkpoint":
            checkpoint += 1
            continue
        args = _arguments(step.get("summary", ""))
        if step.get("tool") == "read_file" and args.get("file_path"):
            path = args["file_path"].lstrip("./")
            if ".themis/" in path:
                evidence += 1
                continue
            key = (path, args.get("offset") or 0, args.get("limit"), args.get("find"))
            reads += 1
            reads_after += checkpoint > 0
            if key in read_at and written_at.get(path, -1) < read_step[key]:
                if read_at[key] < checkpoint:
                    rereads += 1
                else:
                    repeats += 1
            read_at[key], read_step[key] = checkpoint, index
        elif step.get("tool") == "write_file" and args.get("file_path"):
            written_at[args["file_path"].lstrip("./")] = index
        elif step.get("tool") == "apply_patch":
            for path in PATCH_PATH.findall(step.get("summary", "")):
                written_at[path.lstrip("./")] = index
    return {
        "checkpoints": checkpoint, "calls": sum(s["kind"] == "call" for s in steps),
        "failed_calls": sum(s["kind"] == "call" and s.get("ok") is False for s in steps),
        "reads": reads, "reads_after_checkpoint": reads_after, "evidence_reads": evidence,
        "rereads_after_checkpoint": rereads, "repeated_reads": repeats,
        "reread_share": round(rereads / reads_after, 4) if reads_after else None,
        "outputs_written": (len(report["outputs_written"])
                            if report.get("outputs_written") is not None else None),
        # Model requests that did not complete (cut off, failed or cancelled), retried or not.
        "requests": sum(s["kind"] == "request" for s in steps),
        "requests_not_completed": sum(s["kind"] == "request" and (s.get("request") or {}).get("status") != "completed"
                                      for s in steps),
        # Replies cut off at the output limit, and those that were writing a file when cut.
        "cut_offs": sum("cut_off_calls" in f for f in report.get("request_failures") or []),
        "cut_off_writes": sum(any(c.get("tool") in ("write_file", "apply_patch") for c in f.get("cut_off_calls", []))
                              for f in report.get("request_failures") or []),
        # Every output file the task asks for exists at the end.
        "finished": (len(report["outputs_written"]) >= report["expected_outputs"]
                     if report.get("outputs_written") is not None and report.get("expected_outputs") else None),
        **{key: sum((s.get("request") or {}).get(key) or 0 for s in steps if s["kind"] == "request")
           for key in TOKEN_KEYS},
        "duration_s": report.get("duration_s"),
        # How often the runtime's own repeat reminder and loop warning fired.
        "repeat_reminders": report.get("repeat_reminders"),
        "loop_warnings": report.get("loop_warnings"),
        "error": report.get("error"),
    }


TOKEN_KEYS = ("input_tokens", "cached_input_tokens", "output_tokens", "reasoning_output_tokens")


def summarize(rows):
    def stats(key):
        values = [r[key] for r in rows if r[key] is not None]
        if not values:
            return None
        return {"mean": round(statistics.mean(values), 3),
                "sd": round(statistics.stdev(values), 3) if len(values) > 1 else 0.0,
                "min": min(values), "max": max(values)}
    keys = ("checkpoints", "calls", "failed_calls", "reads", "evidence_reads", "rereads_after_checkpoint",
            "repeated_reads", "reread_share", "outputs_written", "repeat_reminders", "loop_warnings",
            "requests", "requests_not_completed", "cut_offs", "cut_off_writes", "finished", *TOKEN_KEYS,
            "duration_s")
    return {"runs": len(rows), "errors": sum(1 for r in rows if r["error"]),
            **{key: stats(key) for key in keys}}
