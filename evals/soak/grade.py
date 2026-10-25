"""Graders: one recorded soak run (its trace) -> every measure in the design's table.

A trace is what the runner recorded (and what the self-tests synthesise):

- `events`: `[{"t": seconds, "run_id": str, "event": ThreadEvent}]`, this chat's events from
  `themis watch` in arrival order;
- `goal_polls`: `[{"t", "goal"}]`, `get_goal` answers while the goal ran;
- `saved_checkpoints`: checkpoint summaries read back with `get_thread_history_page`;
- `goal_text`, `followup` (`{"t", "text"}` or null), `recall` (`[{"id", "run_id", "answer",
  "tools"}]`), `end` (`{"reason", "detail"}`), `final` (`{"files", "check_passed", "commits",
  "dirty"}`), `usage`, `cost`, `wall_s`.

Every measure has a kind: `harness` (a breakage of our machinery: flagged at any occurrence),
`model` (tracked as a trend, flagged only when it leaves its recent range) or `reported`.
"""

import re

from evals.kit.errors import classify_error, is_infrastructure

from . import generate, tool_causes

GOAL_TURN_MARKER = "[Goal — runtime, not from the user]"
ITEM = r"[a-z]+-[a-z]+"
OUT_PATH = re.compile(rf"out/({ITEM})\.md")
SRC_PATH = re.compile(rf"src/({ITEM})\.txt")
WRITE_TOOLS = ("write_file", "apply_patch", "edit_file", "copy_file", "move_file")
CHECKPOINT_FAILURE = re.compile(r"exceeds? the (?:context|request) budget|checkpoint event queue|"
                                r"compaction; originals preserved|summary instructions exceed", re.IGNORECASE)
# End states: harness breakages, model outcomes, and lost-to-infrastructure.
HARNESS_ENDS = ("failed", "crash", "server_hang", "not_continuing")
MODEL_ENDS = ("reached", "stuck", "waiting", "cap_cost", "cap_turns", "cap_time")
CAP_ENDS = ("cap_cost", "cap_turns", "cap_time")

# What each saved checkpoint should keep: the goal and both first-message rules (and, once it
# was sent, the follow-up's format change). Each is kept when every pattern matches.
RETENTION = {
    "goal": (r"INDEX\.md", r"check\.py"),
    "rule_owner": (r"owner\s*:|\bowner\b.{0,60}\b(?:first|start|begin|top)|\b(?:first|start|begin)\w*\b.{0,60}\bowner\b",),
    "rule_retired": (r"retired",),
    "format_change": (r"\bsince\b",),
}
_NEGATION = re.compile(r"\b(?:no|not|never|without|skip|omit|exclude|don't|do not|n't)\b", re.IGNORECASE)

# name -> (area, kind, better): better is "higher", "lower" or None (reported, never flagged).
MEASURES = {
    "goal_continuation_breaks": ("goal", "harness", "lower"),
    "turns_after_reached": ("goal", "harness", "lower"),
    "goal_changed": ("goal", "harness", "lower"),
    "reached": ("goal", "model", "higher"),
    "goal_turns": ("goal", "reported", None),
    "checkpoints": ("compaction", "reported", None),
    "failed_checkpoints": ("compaction", "harness", "lower"),
    "checkpoints_not_shrinking": ("compaction", "harness", "lower"),
    "rule_retention": ("compaction", "model", "higher"),
    "rule_retention_min": ("compaction", "model", "higher"),
    "rule_violations_after_checkpoint": ("memory", "model", "lower"),
    "redone_items": ("memory", "model", "lower"),
    "redone_checkpoint_share": ("memory", "reported", None),
    "skipped_items": ("memory", "model", "lower"),
    "duplicated_items": ("memory", "model", "lower"),
    "rereads_after_checkpoint": ("memory", "model", "lower"),
    "item_accuracy": ("memory", "model", "higher"),
    "owner_accuracy": ("memory", "model", "higher"),
    "index_accuracy": ("memory", "model", "higher"),
    "format_change_applied": ("memory", "model", "higher"),
    "check_passes": ("memory", "model", "higher"),
    "recall_correct": ("memory", "model", "higher"),
    "tool_errors_ours": ("tools", "harness", "lower"),
    "tool_errors_model": ("tools", "model", "lower"),
    "tool_errors_unclassified": ("tools", "harness", "lower"),
    "health_breakage": ("health", "harness", "lower"),
    "failed_runs": ("health", "harness", "lower"),
    "wall_minutes": ("health", "reported", None),
    "requests": ("health", "reported", None),
    "input_tokens": ("health", "reported", None),
    "cached_share": ("health", "reported", None),
    "cost_usd": ("health", "reported", None),
}


def is_goal_turn(text):
    return (text or "").startswith(GOAL_TURN_MARKER) or (text or "").lstrip().startswith("/goal")


def runs(trace):
    """Runs in start order: {run_id, text, start, events, end}. Events with no run (goal and
    queue changes) are left out."""
    order, by_id = [], {}
    for index, item in enumerate(trace.get("events") or []):
        run_id = item.get("run_id") or ""
        if not run_id:
            continue
        if run_id not in by_id:
            by_id[run_id] = {"run_id": run_id, "text": "", "start": index, "events": [], "end": None}
            order.append(by_id[run_id])
        run = by_id[run_id]
        event = item["event"]
        run["events"].append((index, event))
        if event.get("kind") == "user_message" and not run["text"]:
            run["text"] = event.get("text") or ""
        if event.get("kind") in ("finished", "failed", "incomplete"):
            run["end"] = event
    return order


def _flat(trace):
    return [(index, item.get("run_id") or "", item["event"]) for index, item in enumerate(trace.get("events") or [])]


# --- goal -------------------------------------------------------------------------------------

def goal_measures(trace):
    events = _flat(trace)
    goal_runs = [run for run in runs(trace) if is_goal_turn(run["text"])]
    turn_starts = [run["start"] for run in goal_runs]
    end_reason = (trace.get("end") or {}).get("reason")
    notes = [(index, event) for index, _run, event in events if event.get("kind") == "goal_note"]
    breaks = 0
    for position, (index, event) in enumerate(notes):
        if event.get("outcome") != "not_yet":
            continue
        later_turn = any(start > index for start in turn_starts)
        # The run's last note may be cut short by a cap or a stop: that is not a break.
        last = position == len(notes) - 1
        if not later_turn and not (last and end_reason in CAP_ENDS):
            breaks += 1
    reached_at = next((index for index, event in notes if event.get("outcome") == "reached"), None)
    after_reached = 0 if reached_at is None else sum(start > reached_at for start in turn_starts)
    goal_text = trace.get("goal_text") or ""
    # Counted from the first poll that saw the goal: a poll racing the /goal send is not a change.
    changed, seen = 0, False
    for poll in trace.get("goal_polls") or []:
        goal = poll.get("goal")
        if poll.get("phase", "goal") != "goal":
            continue
        seen = seen or goal is not None
        if seen and (goal is None or (goal_text and goal.get("text") != goal_text)):
            changed += 1
    return {"goal_continuation_breaks": breaks, "turns_after_reached": after_reached,
            "goal_changed": changed, "reached": int(reached_at is not None),
            "goal_turns": len(goal_runs)}


# --- compaction -------------------------------------------------------------------------------

def checkpoint_sizes(trace):
    """[(before, after)] context size (estimated used tokens) around each checkpoint: the last
    `context_budget` before its `context_compacting`, the first after it (None when unseen)."""
    sizes, last_budget, before, pending = [], None, None, []
    for _index, _run, event in _flat(trace):
        kind = event.get("kind")
        if kind == "context_budget":
            used = (event.get("budget") or {}).get("used_tokens")
            if pending:
                for slot in pending:
                    sizes[slot] = (sizes[slot][0], used)
                pending = []
            last_budget = used
        elif kind == "context_compacting":
            before = last_budget
        elif kind == "context_checkpoint":
            sizes.append((before if before is not None else last_budget, None))
            pending.append(len(sizes) - 1)
            before = None
    return sizes


def failed_checkpoints(trace):
    failed = 0
    for run in runs(trace):
        pending = False
        for _index, event in run["events"]:
            kind = event.get("kind")
            if kind == "context_compacting":
                pending = True
            elif kind == "context_checkpoint":
                pending = False
            elif kind == "context_suspended":
                failed += 1
                pending = False
            elif kind == "failed":
                if CHECKPOINT_FAILURE.search(event.get("error") or ""):
                    failed += 1
                elif pending:
                    failed += 1
                pending = False
    return failed


def kept(summary, name):
    if name == "rule_retired":
        for match in re.finditer(r"retired", summary, re.IGNORECASE):
            window = summary[max(0, match.start() - 120):match.end() + 120]
            if _NEGATION.search(window) and re.search(r"file|out/|\.md", window, re.IGNORECASE):
                return True
        return False
    return all(re.search(pattern, summary, re.IGNORECASE | re.DOTALL) for pattern in RETENTION[name])


def retention(trace):
    """Per saved checkpoint: which of the goal and rules it kept, and the share kept."""
    summaries = trace.get("saved_checkpoints")
    if not summaries:
        summaries = [event.get("summary") or "" for _i, _r, event in _flat(trace)
                     if event.get("kind") == "context_checkpoint"]
    followup_index = _followup_index(trace)
    checkpoint_indices = [index for index, _r, event in _flat(trace) if event.get("kind") == "context_checkpoint"]
    rows = []
    for position, summary in enumerate(summaries):
        names = ["goal", "rule_owner", "rule_retired"]
        at = checkpoint_indices[position] if position < len(checkpoint_indices) else None
        if followup_index is not None and at is not None and at > followup_index:
            names.append("format_change")
        hits = {name: kept(summary or "", name) for name in names}
        rows.append({"checkpoint": position + 1, "kept": hits, "share": sum(hits.values()) / len(hits)})
    return rows


def compaction_measures(trace):
    sizes = checkpoint_sizes(trace)
    rows = retention(trace)
    shares = [row["share"] for row in rows]
    return {"checkpoints": sum(event.get("kind") == "context_checkpoint" for _i, _r, event in _flat(trace)),
            "failed_checkpoints": failed_checkpoints(trace),
            "checkpoints_not_shrinking": sum(1 for before, after in sizes
                                             if before is not None and after is not None and after >= before),
            "rule_retention": round(sum(shares) / len(shares), 3) if shares else None,
            "rule_retention_min": round(min(shares), 3) if shares else None}


# --- memory -----------------------------------------------------------------------------------

def _followup_index(trace):
    followup = (trace.get("followup") or {}).get("text")
    if not followup:
        return None
    for index, _run, event in _flat(trace):
        if event.get("kind") in ("user_message", "steered") and (event.get("text") or "").strip() == followup.strip():
            return index
    return None


def tool_calls(trace):
    """Each tool call: {index, run_id, tool, summary, ok, output, checkpoint (count before it)}.
    A `tool_finished` closes the oldest open call of the same tool in its run."""
    calls, open_calls, checkpoints = [], {}, 0
    for index, run_id, event in _flat(trace):
        kind = event.get("kind")
        if kind == "context_checkpoint":
            checkpoints += 1
        elif kind == "tool_started":
            call = {"index": index, "run_id": run_id, "tool": event.get("tool"), "summary": event.get("summary") or "",
                    "ok": None, "output": "", "checkpoint": checkpoints}
            calls.append(call)
            open_calls.setdefault((run_id, call["tool"]), []).append(call)
        elif kind == "tool_finished":
            waiting = open_calls.get((run_id, event.get("tool")))
            if waiting:
                call = waiting.pop(0)
                call["ok"], call["output"] = bool(event.get("ok")), event.get("output") or ""
    return calls


def _full_write(call):
    summary = call["summary"]
    return call["tool"] == "write_file" and not re.search(r"append to|\"append\"\s*:\s*true", summary)


def _checks_between(calls, start, end):
    return any(c["tool"] == "shell" and "check.py" in c["summary"] and start < c["index"] < end for c in calls)


def memory_measures(trace, spec):
    items = {item["name"]: item for item in spec["items"]}
    calls = tool_calls(trace)
    followup_index = _followup_index(trace)
    checkpoint_events = [(index, event.get("summary") or "") for index, _r, event in _flat(trace)
                         if event.get("kind") == "context_checkpoint"]
    files = (trace.get("final") or {}).get("files") or {}

    writes = {}  # name -> [call]
    for call in calls:
        if call["ok"] and call["tool"] in WRITE_TOOLS:
            for name in set(OUT_PATH.findall(call["summary"])):
                if name in items:
                    writes.setdefault(name, []).append(call)

    redone = redone_listed = 0
    for name, made in writes.items():
        full = [call for call in made if _full_write(call)]
        for previous, call in zip(full, full[1:]):
            same_phase = followup_index is None or (previous["index"] > followup_index) == (call["index"] > followup_index)
            if not same_phase or _checks_between(calls, previous["index"], call["index"]):
                continue
            redone += 1
            between = [summary for index, summary in checkpoint_events if previous["index"] < index < call["index"]]
            if between and re.search(rf"\b{re.escape(name)}\b", between[-1]):
                redone_listed += 1

    # Rules in files last written after a checkpoint (attributed to that checkpoint's segment).
    segments = {}
    for name, made in writes.items():
        segment = made[-1]["checkpoint"]
        if segment == 0:
            continue
        row = segments.setdefault(segment, {"written": 0, "violations": 0})
        row["written"] += 1
        row["violations"] += int(bool(rule_violations(items[name], files.get(f"out/{name}.md"))))
    after_checkpoint = sum(row["violations"] for row in segments.values())

    active = [name for name, item in items.items() if item["status"] != "retired"]
    skipped = sum(f"out/{name}.md" not in files for name in active)
    strays = [path for path in files if path.startswith("out/") and path != "out/INDEX.md"
              and (OUT_PATH.fullmatch(path) is None or OUT_PATH.fullmatch(path).group(1) not in items)]
    listed = re.findall(rf"^\s*-\s*({ITEM}):", files.get("out/INDEX.md") or "", re.MULTILINE)
    duplicated = len(strays) + sum(listed.count(name) - 1 for name in set(listed) if listed.count(name) > 1)

    # A re-read counts when the item's output was already written, in the same phase (before or
    # after the follow-up, which asks for a new fact from every memo), before an earlier checkpoint.
    def phase(index):
        return followup_index is not None and index > followup_index
    rereads = 0
    for call in calls:
        if call["tool"] != "read_file" or call["checkpoint"] == 0:
            continue
        for name in set(SRC_PATH.findall(call["summary"])):
            done = [w for w in writes.get(name, []) if w["index"] < call["index"] and phase(w["index"]) == phase(call["index"])]
            if done and done[0]["checkpoint"] < call["checkpoint"]:
                rereads += 1

    def share(test, names):
        return round(sum(bool(test(items[n], files.get(f"out/{n}.md"))) for n in names) / len(names), 3) if names else None
    applied = share(generate.since_correct, active) if followup_index is not None else None
    index_text = files.get("out/INDEX.md")

    # A recall answer counts when it matches and came from memory (no tool call in its run).
    recall = trace.get("recall") or []
    correct = sum(bool(answer.get("answer")) and not answer.get("tools")
                  and bool(re.search(answer.get("expect") or r"$^", answer["answer"], re.IGNORECASE))
                  for answer in recall)
    final = trace.get("final") or {}
    return ({"rule_violations_after_checkpoint": after_checkpoint, "redone_items": redone,
             "redone_checkpoint_share": redone_listed, "skipped_items": skipped,
             "duplicated_items": duplicated, "rereads_after_checkpoint": rereads,
             "item_accuracy": share(generate.item_correct, active),
             "owner_accuracy": share(generate.owner_correct, active),
             "index_accuracy": None if index_text is None else round(generate.index_statuses(spec, index_text), 3),
             "format_change_applied": applied,
             "check_passes": None if final.get("check_passed") is None else int(bool(final["check_passed"])),
             "recall_correct": correct if recall else None},
            {"segments": {str(k): v for k, v in sorted(segments.items())},
             "final_rule_violations": sum(bool(rule_violations(items[n], files.get(f"out/{n}.md"))) for n in items)})


def rule_violations(item, text):
    """The rules an item's final output breaks: `owner` (the first line is not an `owner: <team>`
    line: the rule was forgotten; naming the wrong team is `owner_accuracy`) and `retired` (a
    retired item has a file)."""
    broken = []
    if item["status"] == "retired":
        if text is not None:
            broken.append("retired")
        return broken
    if text is not None and not re.fullmatch(r"owner:\s*team-[a-z]+", (text.splitlines() or [""])[0].strip()):
        broken.append("owner")
    return broken


# --- tools and health -------------------------------------------------------------------------

def tool_measures(trace):
    counts = {"ours": 0, "model": 0, "unclassified": 0}
    unclassified, ours = [], []
    for call in tool_calls(trace):
        if call["ok"] is not False:
            continue
        cause, code = tool_causes.classify(call["tool"], call["output"], trace.get("mode", "yolo"))
        counts[cause] += 1
        if cause == "unclassified":
            unclassified.append({"tool": call["tool"], "error": tool_causes.sample(call["output"])})
        elif cause == "ours":
            ours.append({"tool": call["tool"], "code": code, "error": tool_causes.sample(call["output"])})
    for _index, _run, event in _flat(trace):
        if event.get("kind") == "approval_decided" and event.get("decision") in ("deny", "denied") \
                and trace.get("mode", "yolo") == "yolo":
            counts["ours"] += 1
            ours.append({"tool": event.get("tool"), "code": "approval_in_yolo", "error": ""})
    return ({"tool_errors_ours": counts["ours"], "tool_errors_model": counts["model"],
             "tool_errors_unclassified": counts["unclassified"]},
            {"unclassified": unclassified, "ours": ours})


def failed_runs(trace):
    """Runs that failed for a reason that is neither the provider nor our own stop."""
    count = 0
    for run in runs(trace):
        end = run["end"]
        if not end or end.get("kind") != "failed":
            continue
        error_class = classify_error(end.get("error"))
        if is_infrastructure(error_class) or (error_class or "").startswith("stopped"):
            continue
        count += 1
    return count


def infrastructure(trace):
    """Whether the run was lost to the provider: it did not reach its goal and the last thing
    that failed was a provider request (rate limit, outage, cut-off reply), or the runner
    recorded an infrastructure end."""
    end = trace.get("end") or {}
    if end.get("reason") == "infrastructure":
        return True
    if end.get("reason") == "reached":
        return False
    failures = [run["end"] for run in runs(trace) if run["end"] and run["end"].get("kind") == "failed"]
    if failures and is_infrastructure(classify_error(failures[-1].get("error"))):
        return True
    notes = [event for _i, _r, event in _flat(trace) if event.get("kind") == "goal_note"]
    return bool(notes) and notes[-1].get("outcome") == "quota"


def health_measures(trace):
    usage = trace.get("usage") or {}
    inputs = usage.get("input_tokens") or 0
    cached = usage.get("cached_input_tokens") or 0
    reason = (trace.get("end") or {}).get("reason")
    return {"health_breakage": int(reason in HARNESS_ENDS), "failed_runs": failed_runs(trace),
            "wall_minutes": round((trace.get("wall_s") or 0) / 60, 1), "requests": usage.get("requests"),
            "input_tokens": inputs or None, "cached_share": round(cached / inputs, 3) if inputs else None,
            "cost_usd": None if trace.get("cost") is None else round(trace["cost"], 4)}


def grade(trace, spec):
    """(measures, details) for one run."""
    measures = {}
    measures.update(goal_measures(trace))
    measures.update(compaction_measures(trace))
    memory, memory_details = memory_measures(trace, spec)
    measures.update(memory)
    tools, tool_details = tool_measures(trace)
    measures.update(tools)
    measures.update(health_measures(trace))
    details = {"end_state": (trace.get("end") or {}).get("reason"), "end_detail": (trace.get("end") or {}).get("detail"),
               "retention": retention(trace), "checkpoint_sizes": checkpoint_sizes(trace),
               **memory_details, **tool_details}
    return measures, details
