"""Aggregates graded trials into the results JSON."""

import statistics

from .grade import position

SCORED = ("correct", "unknown", "stale", "wrong", "error")


def spread(values):
    """mean, sample standard deviation, min, max (None when there is nothing to average)."""
    if not values:
        return None
    return {
        "mean": round(statistics.fmean(values), 4),
        "sd": round(statistics.stdev(values), 4) if len(values) > 1 else 0.0,
        "min": round(min(values), 4),
        "max": round(max(values), 4),
        "n": len(values),
    }


def _rate(count, total):
    return count / total if total else None


def trial_rows(case, run):
    """Graded rows for one run: one dict per probe."""
    from .grade import grade

    answers = {a["id"]: a["answer"] for a in run["answers"]}
    rows = []
    for probe in case["probes"]:
        verdict = grade(probe, answers.get(probe["id"]))
        rows.append({
            "case": case["name"],
            "id": probe["id"],
            "category": probe["category"],
            "position": position(probe["planted_at"], case["records"]),
            "has_stale": bool(probe.get("stale")),
            "verdict": verdict,
            "answer": answers.get(probe["id"]),
        })
    return rows


def _retention(rows, key=None, value=None):
    scored = [r for r in rows if r["verdict"] in SCORED and r["category"] != "decoy"
              and (key is None or r[key] == value)]
    return _rate(sum(r["verdict"] == "correct" for r in scored), len(scored))


def trial_summary(rows, runs):
    """Metrics for one trial index pooled over all cases."""
    stale_rows = [r for r in rows if r["has_stale"] and r["verdict"] in SCORED]
    decoys = [r for r in rows if r["category"] == "decoy"]
    chunks = [c for run in runs for c in run["chunks"]]
    return {
        "retention": _retention(rows),
        "by_category": {c: _retention(rows, "category", c)
                        for c in sorted({r["category"] for r in rows} - {"decoy"})},
        "by_position": {p: _retention(rows, "position", p) for p in ("early", "middle", "late")},
        "stale_fact_rate": _rate(sum(r["verdict"] == "stale" for r in stale_rows), len(stale_rows)),
        "forgot_rate": _rate(sum(r["verdict"] == "unknown" for r in rows if r["category"] != "decoy"),
                             sum(r["category"] != "decoy" for r in rows)),
        "wrong_answers": sum(r["verdict"] == "wrong" for r in rows),
        "invented_decoy_answers": sum(r["verdict"] == "invented" for r in decoys),
        "decoy_probes": len(decoys),
        "probe_errors": sum(r["verdict"] == "error" for r in rows),
        "compactions": len(chunks),
        "compactions_failed": sum(not c["installed"] for c in chunks),
        "compaction_ms_mean": statistics.fmean([c["duration_ms"] for c in chunks]) if chunks else None,
        "tokens_before_mean": statistics.fmean([c["tokens_before"] for c in chunks]) if chunks else None,
        "tokens_after_mean": statistics.fmean([c["tokens_after"] for c in chunks]) if chunks else None,
        "provider_requests_per_compaction": statistics.fmean([c["requests"] for c in chunks]) if chunks else None,
    }


def _collect(trials, path):
    out = []
    for trial in trials:
        value = trial
        for part in path:
            value = value.get(part) if isinstance(value, dict) else None
        if value is not None:
            out.append(value)
    return out


def aggregate(trials):
    """Mean +- spread across trials for every metric in `trial_summary` outputs."""
    scalar = ["retention", "stale_fact_rate", "forgot_rate", "wrong_answers", "invented_decoy_answers",
              "probe_errors", "compactions", "compactions_failed", "compaction_ms_mean",
              "tokens_before_mean", "tokens_after_mean", "provider_requests_per_compaction"]
    result = {name: spread(_collect(trials, [name])) for name in scalar}
    result["by_category"] = {c: spread(_collect(trials, ["by_category", c]))
                             for c in sorted({c for t in trials for c in t["by_category"]})}
    result["by_position"] = {p: spread(_collect(trials, ["by_position", p]))
                             for p in ("early", "middle", "late")}
    return result
