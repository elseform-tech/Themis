"""Rows -> per-trial metrics -> mean +- SD across trials, and the markdown summary."""

import statistics

from evals.compaction.report import spread

PROVIDER_FAILURE = ("LLM request failed", "Provider", "timed out", "rate limit", "429", "connection")


def is_infrastructure_error(row):
    """A provider/transport failure (excluded from accuracy); a run that merely failed to answer is not."""
    error = row.get("error") or ""
    return row["verdict"] == "error" or (not row["answer"] and any(p.lower() in error.lower() for p in PROVIDER_FAILURE))


def _rate(count, total):
    return count / total if total else None


def _mean(values):
    values = [v for v in values if v is not None]
    return statistics.fmean(values) if values else None


def accuracy_block(rows):
    """Metrics over original-only (not in context) non-decoy probes of one group."""
    n = len(rows)
    correct = [r for r in rows if r["verdict"] == "correct"]
    return {
        "probes": n,
        "headline_correct_with_valid_citation": _rate(sum(r["verdict"] == "correct" and r["cite_valid"] for r in rows), n),
        "accuracy_no_citation_requirement": _rate(len(correct), n),
        "citation_present_rate": _rate(sum(r["cite_present"] for r in rows), n),
        "citation_valid_rate_of_correct": _rate(sum(r["cite_valid"] for r in correct), len(correct)),
        "unknown_rate": _rate(sum(r["verdict"] == "unknown" for r in rows), n),
        "stale_or_wrong_rate": _rate(sum(r["verdict"] in ("stale", "wrong") for r in rows), n),
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
        "by_category": {c: accuracy_block([r for r in original if r["category"] == c])["accuracy_no_citation_requirement"]
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
             "| Group | Original-only probes | Correct + valid citation (headline) | Correct, no citation requirement | Citation valid (of correct) | Unknown |",
             "| --- | --- | --- | --- | --- | --- |"]
    for label, block in (("narrative", agg["by_domain"].get("narrative")), ("code", agg["by_domain"].get("code")),
                         ("all", agg["overall"])):
        block = block or {}
        block = {k: block.get(k) for k in ("probes", "headline_correct_with_valid_citation", "accuracy_no_citation_requirement",
                                           "citation_valid_rate_of_correct", "unknown_rate")}
        lines.append(f"| {label} | {fmt(block['probes'], False)} | **{fmt(block['headline_correct_with_valid_citation'])}** | "
                     f"{fmt(block['accuracy_no_citation_requirement'])} | {fmt(block['citation_valid_rate_of_correct'])} | {fmt(block['unknown_rate'])} |")
    lines += ["", "| Other metric | Value |", "| --- | --- |",
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
