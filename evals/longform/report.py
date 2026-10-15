"""Rows -> per-trial metrics -> mean +- SD across trials, and the markdown summary."""

import re
import statistics

from evals.compaction.report import spread

# Redacted error classes (never provider bodies); the first two are request failures, not agent failures.
INFRASTRUCTURE_CLASSES = ("provider_request", "provider_response", "timeout")


def classify_error(text):
    """Reduces raw error text to a class and an HTTP status; nothing else is kept (no bodies, no paths)."""
    if not text:
        return None
    lowered = text.lower()
    status = re.search(r"\b(?:http|status|code)\D{0,6}([45]\d\d)\b|\b([45]\d\d)\b(?= (?:too many|bad|unauthor|forbidden|not found|internal|service))", lowered)
    status = next((g for g in status.groups() if g), None) if status else None
    for name, needles in (("timeout", ("timed out", "timeout")),
                          ("provider_response", ("response format error", "decode", "truncated")),
                          ("provider_request", ("llm request failed", "provider", "rate limit", "connection", "request failed")),
                          ("tool_loop", ("repeated unchanged tool results",)),
                          ("stopped", ("stopped by you",)),
                          ("turn_limit", ("max turns", "turn limit", "maximum number of turns"))):
        if any(n in lowered for n in needles):
            return f"{name}:{status}" if status else name
    return "other"


def is_infrastructure_error(row):
    """A provider/transport failure (excluded from accuracy); a run that merely failed to answer is not."""
    error = row.get("error") or ""
    return row["verdict"] == "error" or (not row["answer"] and error.startswith(INFRASTRUCTURE_CLASSES))


def _rate(count, total):
    return count / total if total else None


def _mean(values):
    values = [v for v in values if v is not None]
    return statistics.fmean(values) if values else None


def _supported(row):
    """Grounded in what the agent read; results from before tool outputs were recorded fall back
    to the old answer-citation check."""
    value = row.get("grounded")
    return row.get("cite_valid", False) if value is None else value


def accuracy_block(rows):
    """Metrics over original-only (not in context) non-decoy probes of one group."""
    n = len(rows)
    correct = [r for r in rows if r["verdict"] == "correct"]
    return {
        "probes": n,
        "headline_correct_and_grounded": _rate(sum(r["verdict"] == "correct" and _supported(r) for r in rows), n),
        "accuracy_no_grounding_requirement": _rate(len(correct), n),
        "grounded_rate_of_correct": _rate(sum(_supported(r) for r in correct), len(correct)),
        "source_leak_rate": _rate(sum(r.get("leak", False) for r in rows), n),
        "citation_present_rate": _rate(sum(r["cite_present"] for r in rows), n),
        "citations_over_cap_rate": _rate(sum(r.get("cite_over_cap", False) for r in rows), n),
        "mean_citations_per_answer": _mean(r.get("cite_count", 0) for r in rows if r["cite_present"]),
        "hedged_rate": _rate(sum(r["verdict"] == "hedged" for r in rows), n),
        "unknown_rate": _rate(sum(r["verdict"] == "unknown" for r in rows), n),
        "stale_or_wrong_rate": _rate(sum(r["verdict"] in ("stale", "wrong", "hedged") for r in rows), n),
    }


def trial_summary(rows, runs):
    usable = [r for r in rows if not is_infrastructure_error(r)]
    detail = [r for r in usable if r["category"] != "decoy"]
    original = [r for r in detail if r["in_context"] is None]
    by_domain = {d: accuracy_block([r for r in original if r["domain"] == d]) for d in ("narrative", "code")}
    quotes = [r for r in original if r["category"] == "quote"]
    decoys = [r for r in usable if r["category"] == "decoy"]
    in_context = [r for r in detail if r["in_context"] is not None]
    asked = [r for r in usable]
    chunks = [c for run in runs for c in run["chunks"]]
    return {
        "overall": accuracy_block(original),
        "by_domain": by_domain,
        "by_category": {c: accuracy_block([r for r in original if r["category"] == c])["accuracy_no_grounding_requirement"]
                        for c in sorted({r["category"] for r in original})},
        "quote_exact_accuracy": _rate(sum(r["verdict"] == "correct" for r in quotes), len(quotes)),
        "quote_probes": len(quotes),
        "decoy_hallucination_rate": _rate(sum(r["verdict"] == "invented" for r in decoys), len(decoys)),
        "decoy_probes": len(decoys),
        "excluded_in_context": len(in_context),
        "excluded_in_context_checkpoint": sum(r["in_context"] == "checkpoint" for r in in_context),
        "excluded_in_context_tail": sum(r["in_context"] == "tail" for r in in_context),
        "in_context_accuracy": _rate(sum(r["verdict"] == "correct" for r in in_context), len(in_context)),
        "mean_tool_calls": _mean(r["tool_calls"] for r in asked),  # attempted, including refused calls
        "mean_executed_tool_calls": _mean(r.get("executed_tool_calls", r["tool_calls"]) for r in asked),
        "probes_with_refused_calls": _rate(sum(r["denied"] > 0 for r in asked), len(asked)),
        "recall_used_rate": _rate(sum(r["used_recall"] for r in asked), len(asked)),
        "recall_available": any(run.get("recall_available") for run in runs),
        "mean_requests": _mean(r["requests"] for r in asked),
        "mean_input_tokens": _mean(r["input_tokens"] for r in asked),
        "mean_output_tokens": _mean(r["output_tokens"] for r in asked),
        "mean_latency_s": _mean(r["latency_ms"] / 1000 for r in asked),
        "probe_infrastructure_errors": len(rows) - len(usable),
        "original_only_probes": len(original),
        "compactions": len(chunks),
        "compactions_failed": sum(not c["installed"] for c in chunks),
        "compaction_tokens_after_mean": _mean(c["tokens_after"] for c in chunks),
    }


def _walk(trials, path):
    out = []
    for trial in trials:
        value = trial
        for part in path:
            value = value.get(part) if isinstance(value, dict) else None
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            out.append(value)
    return out


def _paths(value, prefix=()):
    if isinstance(value, dict):
        for key, item in value.items():
            yield from _paths(item, prefix + (key,))
    elif isinstance(value, (int, float)) and not isinstance(value, bool):
        yield prefix


def aggregate(trials):
    """Same shape as `trial_summary`, with every number replaced by mean/sd/min/max/n."""
    result = {}
    for path in sorted({p for trial in trials for p in _paths(trial)}):
        node = result
        for part in path[:-1]:
            node = node.setdefault(part, {})
        node[path[-1]] = spread(_walk(trials, path))
    result["recall_available"] = any(t.get("recall_available") for t in trials)
    return result


def fmt(stat, percent=True):
    stat = stat if isinstance(stat, dict) else None
    if not stat:
        return "n/a"
    scale = 100 if percent else 1
    digits = 1 if percent else 2
    suffix = "%" if percent else ""
    return f"{stat['mean'] * scale:.{digits}f}{suffix} ± {stat['sd'] * scale:.{digits}f}"


def markdown(summary):
    agg = summary["aggregate"]
    if not agg:
        return "No valid trials.\n"
    lines = [f"# Long-form recall: {summary.get('model')} @ {summary.get('budget')} tokens",
             "", f"Trials with data: {summary['trials_with_data']}/{summary['trials_requested']}; "
             f"invalid runs: {len(summary['invalid_runs'])}; failed runs: {len(summary['failed_runs'])}. Mean ± SD over trials.", "",
             "| Group | Original-only probes | Correct + grounded (headline) | Correct, no grounding requirement | Grounded (of correct) | Unknown | Source leaked |",
             "| --- | --- | --- | --- | --- | --- | --- |"]
    for label, block in (("narrative", agg["by_domain"].get("narrative")), ("code", agg["by_domain"].get("code")),
                         ("all", agg["overall"])):
        block = block or {}
        block = {k: block.get(k) for k in ("probes", "headline_correct_and_grounded", "accuracy_no_grounding_requirement",
                                           "grounded_rate_of_correct", "unknown_rate", "source_leak_rate")}
        lines.append(f"| {label} | {fmt(block['probes'], False)} | **{fmt(block['headline_correct_and_grounded'])}** | "
                     f"{fmt(block['accuracy_no_grounding_requirement'])} | {fmt(block['grounded_rate_of_correct'])} | {fmt(block['unknown_rate'])} | "
                     f"{fmt(block['source_leak_rate'])} |")
    lines += ["", "| Other metric | Value |", "| --- | --- |",
              f"| Hedged answers (two planted values; counted wrong) / citations over the cap of 3 | {fmt(agg['overall']['hedged_rate'])} / {fmt(agg['overall']['citations_over_cap_rate'])} |",
              f"| Exact-quote accuracy (original-only) | {fmt(agg.get('quote_exact_accuracy'))} |",
              f"| Decoy hallucination rate | {fmt(agg.get('decoy_hallucination_rate'))} |",
              f"| Probes excluded as in-context (checkpoint / tail) | {fmt(agg['excluded_in_context'], False)} ({fmt(agg['excluded_in_context_checkpoint'], False)} / {fmt(agg['excluded_in_context_tail'], False)}) |",
              f"| Accuracy on in-context probes (secondary) | {fmt(agg.get('in_context_accuracy'))} |",
              f"| Mean tool calls per probe (attempted / executed) | {fmt(agg['mean_tool_calls'], False)} / {fmt(agg['mean_executed_tool_calls'], False)} |",
              f"| Probes with a refused tool call (e.g. python3, writes) | {fmt(agg['probes_with_refused_calls'])} |",
              f"| `recall` available / used (share of probes) | {agg['recall_available']} / {fmt(agg['recall_used_rate'])} |",
              f"| Mean provider requests per probe | {fmt(agg['mean_requests'], False)} |",
              f"| Mean input / output tokens per probe | {fmt(agg['mean_input_tokens'], False)} / {fmt(agg['mean_output_tokens'], False)} |",
              f"| Mean latency per probe (s) | {fmt(agg['mean_latency_s'], False)} |",
              f"| Probe infrastructure errors per trial (excluded) | {fmt(agg['probe_infrastructure_errors'], False)} |",
              ""]
    return "\n".join(lines)
