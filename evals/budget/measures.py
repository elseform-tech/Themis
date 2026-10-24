"""One result row per run, and the summaries: mean ± sd per size and budget, and the ceiling.

Re-reads after a checkpoint come from `evals.hard.moves` (the first read of a file after a
checkpoint that the run had read or written before it); their share is over the reads made
after the first checkpoint. Runs stopped by a kill have no tool-call record, so their call
measures are unknown (n/a), not zero.
"""

import collections
import math
import statistics

from evals.hard import moves

from . import spend

FAILURE_KINDS = ("stopped: cost", "timeout", "provider error", "cut-off", "turn limit", "wrong answer")


def failure_kind(outcome):
    """Why a run failed, or None when it passed."""
    if outcome.get("stopped") == "cost":
        return "stopped: cost"
    if outcome.get("stopped") == "timeout":
        return "timeout"
    error = (outcome.get("error") or "").lower()
    if "truncated" in error or "cut off" in error:
        return "cut-off"
    if "max turns" in error or error.startswith("incomplete"):
        return "turn limit"
    if error and any(word in error for word in ("llm request failed", "provider", "rate limit", "http",
                                                "connection", "stream")):
        return "provider error"
    if outcome["grade"]["passed"] and outcome.get("finished"):
        return None
    return "wrong answer"


def reread_measures(calls, checkpoints):
    if calls is None or checkpoints is None:
        return {"rereads_after_checkpoint": None, "reads_after_checkpoint": None, "reread_share": None}
    row = {"calls": calls, "checkpoints": checkpoints}
    rereads = moves.rereads_after_checkpoints(row)
    first = min((c["calls_before"] for c in checkpoints), default=None)
    after = 0 if first is None else sum(c.get("tool") == "read_file" for c in calls[first:])
    return {"rereads_after_checkpoint": rereads, "reads_after_checkpoint": after,
            "reread_share": round(rereads / after, 4) if after else None}


def make_row(arm, outcome, price):
    usage = outcome["usage"]
    grade = outcome["grade"]
    kind = failure_kind(outcome)
    inputs = usage.get("input_tokens") or 0
    return {
        "arm": arm["name"], "size": arm["size"], "budget": arm["budget"], "requested_budget": arm["requested"],
        "trial": arm["trial"], "seed": arm["seed"], "via": arm["via"],
        "passed": kind is None, "failure": kind, "why": grade["why"], "stages": grade["stages"],
        "stages_passed": grade["stages_passed"], "desks_passed": grade["desks_passed"], "desks": grade["desks"],
        "failed_desks": grade["failed_desks"], "own_tests": grade["own_tests"],
        "material_tokens": outcome["material_tokens"],
        "input_tokens": inputs, "cached_input_tokens": usage.get("cached_input_tokens") or 0,
        "cached_share": round((usage.get("cached_input_tokens") or 0) / inputs, 4) if inputs else None,
        "output_tokens": usage.get("output_tokens") or 0,
        "reasoning_output_tokens": usage.get("reasoning_output_tokens") or 0,
        "requests": usage.get("requests") or 0,
        "cost_usd": round(spend.cost(price, usage), 4),
        "wall_s": outcome["wall_s"], "compactions": outcome.get("compactions"),
        "calls": None if outcome.get("calls") is None else len(outcome["calls"]),
        "turn_calls": outcome.get("turn_calls"),
        **reread_measures(outcome.get("calls"), outcome.get("checkpoints")),
        "stopped": outcome.get("stopped"), "answer": outcome.get("answer", ""),
        "error_text": (outcome.get("error") or "")[:300],
    }


MEASURES = (("passed", "Pass"), ("stages_passed", "Stages (of 3)"), ("input_tokens", "Input tokens"),
            ("cached_share", "Cached share"), ("output_tokens", "Output tokens"),
            ("reasoning_output_tokens", "Reasoning tokens"), ("cost_usd", "Cost ≈ $"), ("wall_s", "Minutes"),
            ("requests", "Requests"), ("compactions", "Compactions"), ("reread_share", "Re-read share"))


def mean_sd(values):
    values = [float(v) for v in values if v is not None]
    if not values:
        return None
    return {"mean": statistics.fmean(values), "sd": statistics.stdev(values) if len(values) > 1 else 0.0,
            "n": len(values)}


def summarize(rows):
    """{size: {budget: {measure: mean ± sd, failures: {kind: count}}}}."""
    cells = collections.defaultdict(list)
    for row in rows:
        cells[(row["size"], row["budget"])].append(row)
    out = {}
    for (size, budget), runs in sorted(cells.items(), key=lambda item: (SIZE_ORDER.get(item[0][0], 9), item[0][1])):
        cell = {key: mean_sd([r.get(key) for r in runs]) for key, _ in MEASURES}
        if cell["wall_s"]:
            cell["wall_s"] = {k: (v / 60 if k != "n" else v) for k, v in cell["wall_s"].items()}
        cell["runs"] = len(runs)
        cell["failures"] = dict(collections.Counter(r["failure"] for r in runs if r["failure"]))
        out.setdefault(size, {})[str(budget)] = cell
    return out


SIZE_ORDER = {"small": 0, "medium": 1, "large": 2}


def standard_error(cell):
    passed = cell["passed"]
    return passed["sd"] / math.sqrt(passed["n"]) if passed and passed["n"] > 1 else 0.0


def ceiling(summary, size="large"):
    """The design's rule: the smallest budget whose pass rate is within one standard error of the
    best; a larger budget in that range wins only when it ties on pass rate and is both cheaper
    and faster. Returns (budget or None, explanation)."""
    cells = {int(b): c for b, c in (summary.get(size) or {}).items() if c.get("passed")}
    if not cells:
        return None, f"no {size} runs"
    best_budget = max(cells, key=lambda b: (cells[b]["passed"]["mean"], -b))
    best = cells[best_budget]["passed"]["mean"]
    se = standard_error(cells[best_budget])
    within = sorted(b for b, c in cells.items() if c["passed"]["mean"] >= best - se - 1e-12)
    chosen = within[0]

    def mean(budget, key):
        value = cells[budget].get(key)
        return value["mean"] if value else math.inf
    for budget in within[1:]:
        if (cells[budget]["passed"]["mean"] >= cells[chosen]["passed"]["mean"]
                and mean(budget, "cost_usd") < mean(chosen, "cost_usd")
                and mean(budget, "wall_s") < mean(chosen, "wall_s")):
            chosen = budget
    why = (f"best pass rate {best:.0%} at {best_budget}; one standard error {se:.2f}; "
           f"budgets within it: {', '.join(map(str, within))}; chosen {chosen}")
    return chosen, why


def only_large_matters(summary, floor=128_000):
    """Whether small and medium pass at every budget of `floor` and above (design step 3);
    None when either size has no such runs."""
    verdicts = []
    for size in ("small", "medium"):
        cells = [c for b, c in (summary.get(size) or {}).items() if int(b) >= floor and c.get("passed")]
        if not cells:
            return None
        verdicts.append(all(c["passed"]["mean"] == 1.0 for c in cells))
    return all(verdicts)
