"""Grades behavior evals: what the agent does with the next user message after a compaction.

A turn is graded from the reply text and the attempted tool calls reported by the
`behavior_eval` example. Grading is lexical, like the retention grader. Kinds:

- `greeting`: short reply, no tool calls, no mention of the unfinished task (`markers`).
- `unrelated`: the question is answered (`must_match`), no tool calls, task untouched.
- `return`: the user names an earlier topic; score = share of `facts` present in the reply.
- `continue`: a vague continuation resumes the latest task (`resume` matches the reply or a tool
  call) and does not drift to an older topic (`wrong_topic`).
- `no_redo`: resuming after a compaction does not re-read sources the checkpoint already records.
  Each tool call is `progress` (a write whose call matches `resume`), `redo` (any other call matching
  `redo`) or `other`. The first call is the headline: the eval denies every approval and starts from
  an empty folder, so later calls react to that rather than to the checkpoint.
"""

WRITES = r"^(write_file|apply_patch)\b|cat\s*>|\btee\b"


def classify_call(call, resume, redo):
    if _hit(WRITES, call) and _hit(resume, call):
        return "progress"
    return "redo" if _hit(redo, call) else "other"

import re


def _hit(pattern, text):
    return bool(pattern) and re.search(pattern, text or "") is not None


def grade_turn(case, spec, turn):
    """Returns a dict with `pass`, `resumed` (unwanted resume) and the raw counts."""
    reply = turn.get("reply") or ""
    calls = turn.get("tool_calls") or []
    joined = reply + "\n" + "\n".join(calls)
    kind = spec["kind"]
    row = {"id": spec["id"], "kind": kind, "tool_calls": len(calls),
           "compactions": turn.get("compactions", 0), "error": turn.get("error"),
           "reply_chars": len(reply), "resumed": None, "facts_found": None, "facts_total": None}
    if kind in ("greeting", "unrelated"):
        row["resumed"] = bool(calls) or _hit(case.get("markers"), reply)
        short = len(reply) <= case.get("max_chars", 300)
        answered = all(_hit(p, reply) for p in spec.get("must_match", []))
        row["pass"] = (not row["resumed"]) and short and answered and bool(reply.strip()) \
            and row["compactions"] == 0
    elif kind == "return":
        facts = spec["facts"]
        found = sum(_hit(p, reply) for p in facts)
        row.update(facts_found=found, facts_total=len(facts))
        row["pass"] = found == len(facts)
    elif kind == "continue":
        resumed_latest = _hit(spec["resume"], joined)
        drifted = _hit(spec.get("wrong_topic"), joined)
        row["resumed"] = resumed_latest and not drifted
        row["pass"] = row["resumed"]
    elif kind == "no_redo":
        kinds = [classify_call(call, spec["resume"], spec["redo"]) for call in calls]
        row.update(first_call=kinds[0] if kinds else None, redo_calls=kinds.count("redo"),
                   progress=_hit(spec["resume"], joined))
        row["pass"] = row["progress"] and row["first_call"] != "redo"
        # Re-reading a fact the checkpoint lost is legitimate; `retained` says whether it kept them.
        if spec.get("retained"):
            row["retained"] = all(_hit(p, turn.get("checkpoint") or "") for p in spec["retained"])
    else:
        raise ValueError(f"unknown behavior kind {kind!r}")
    return row


def grade_run(case, report):
    specs = {t["id"]: t for t in case["behavior"]["turns"]}
    rows = []
    for turn in report["turns"]:
        spec = specs[turn["id"]]
        turn = dict(turn, checkpoint=report.get("checkpoint"))
        merged = dict(case["behavior"], **{k: v for k, v in spec.items() if k in ("max_chars", "markers")})
        rows.append(grade_turn(merged, spec, turn))
    return rows


def summarize(rows):
    """Aggregate counts over graded turns from every case and trial."""
    def of(kind):
        return [r for r in rows if r["kind"] == kind]
    greet, unrel, back, cont = of("greeting"), of("unrelated"), of("return"), of("continue")
    redo = of("no_redo")
    with_calls = [r for r in redo if r["first_call"]]
    kept = [r for r in with_calls if r.get("retained")]
    unwanted = [r for r in greet + unrel if r["resumed"]]
    facts_total = sum(r["facts_total"] for r in back)
    return {
        "turns": len(rows),
        "greeting_turns": len(greet),
        "greeting_tool_calls": sum(r["tool_calls"] for r in greet),
        "greeting_compactions": sum(r["compactions"] for r in greet),
        "greeting_pass_rate": _rate([r["pass"] for r in greet]),
        "unrelated_pass_rate": _rate([r["pass"] for r in unrel]),
        "unwanted_resume_rate": _rate([True] * len(unwanted) + [False] * (len(greet + unrel) - len(unwanted))),
        "topic_return_fact_retention": round(sum(r["facts_found"] for r in back) / facts_total, 4) if facts_total else None,
        "topic_return_pass_rate": _rate([r["pass"] for r in back]),
        "continue_resume_rate": _rate([r["pass"] for r in cont]),
        "no_redo_pass_rate": _rate([r["pass"] for r in redo]),
        "first_call_redo_rate": _rate([r["first_call"] == "redo" for r in with_calls]),
        "first_call_progress_rate": _rate([r["first_call"] == "progress" for r in with_calls]),
        "redo_calls_per_turn": round(sum(r["redo_calls"] for r in redo) / len(redo), 2) if redo else None,
        "facts_retained_rate": _rate([r.get("retained") for r in redo if "retained" in r]) if any("retained" in r for r in redo) else None,
        "first_call_redo_rate_when_retained": _rate([r["first_call"] == "redo" for r in kept]),
        "errors": sum(1 for r in rows if r["error"]),
    }


def _rate(values):
    return round(sum(bool(v) for v in values) / len(values), 4) if values else None
