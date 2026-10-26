"""History and trend for live evals: each run appends one record; a run is compared with the
recent runs of its series.

- baseline: the median of the last 5 recorded runs of the series before this one, leaving out
  runs lost to infrastructure;
- a model measure is flagged when it is worse than every one of those 5 runs; with fewer than
  5, values are shown without flags;
- a harness measure is flagged at any occurrence (a count above 0), however short the history.

A suite passes its own measures (`name -> (area, kind, better)`) and series key. Nothing is
"pass" or "fail": a flag asks a person to look.
"""

import json
import os
import statistics

WINDOW = 5
SPARKS = "▁▂▃▄▅▆▇█"


def load(path):
    if not path or not os.path.isfile(path):
        return []
    records = []
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if line:
                try:
                    records.append(json.loads(line))
                except ValueError:
                    continue
    return records


def append(path, record):
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(record, sort_keys=True) + "\n")


def prior_runs(history, record, series_key):
    """Earlier counted runs of the record's series, oldest first (the record itself excluded)."""
    key = series_key(record)
    earlier = []
    for other in history:
        if other is record or (record.get("id") and other.get("id") == record.get("id")):
            break
        if series_key(other) == key and not other.get("infrastructure"):
            earlier.append(other)
    return earlier


def worse(value, other, better):
    return value < other if better == "higher" else value > other


def assess(measure, value, previous):
    """{value, baseline, low, high, flag, recent} for one measure (area, kind, better)."""
    _area, kind, better = measure
    recent = [v for v in previous if v is not None][-WINDOW:]
    row = {"value": value, "baseline": None, "low": None, "high": None, "flag": "", "recent": recent}
    if recent:
        row["baseline"] = statistics.median(recent)
        row["low"], row["high"] = min(recent), max(recent)
    if value is None:
        return row
    if kind == "harness" and value > 0:
        row["flag"] = "breakage"
    elif kind == "model" and len(recent) >= WINDOW and all(worse(value, other, better) for other in recent):
        row["flag"] = "regression"
    return row


def compare(history, record, measures, series_key):
    """Every measure of `record` assessed against its series' earlier runs."""
    earlier = prior_runs(history, record, series_key)
    rows = {}
    for name, measure in measures.items():
        previous = [(run.get("measures") or {}).get(name) for run in earlier]
        rows[name] = assess(measure, (record.get("measures") or {}).get(name), previous)
    return rows, len(earlier)


def sparkline(values):
    values = [v for v in values if v is not None]
    if not values:
        return ""
    low, high = min(values), max(values)
    if high == low:
        return SPARKS[3] * len(values)
    return "".join(SPARKS[round((v - low) / (high - low) * (len(SPARKS) - 1))] for v in values)


def fmt(value):
    if value is None:
        return "–"
    if isinstance(value, float):
        if value.is_integer():
            return str(int(value))
        return f"{value:.3g}" if abs(value) < 1000 else str(round(value))
    return str(value)


def table(history, record, measures, areas, series_key, series_text):
    """The markdown measure table for one record; `series_text` names its series."""
    rows, count = compare(history, record, measures, series_key)
    lines = [f"Series: {series_text}; {count} earlier run(s)"
             + ("" if count >= WINDOW else f" (fewer than {WINDOW}: model measures are not flagged yet)"), "",
             "| Area | Measure | Kind | This run | Baseline | Range | Flag | Recent |",
             "| --- | --- | --- | --- | --- | --- | --- | --- |"]
    for area in areas:
        for name, (measure_area, kind, _better) in measures.items():
            if measure_area != area:
                continue
            row = rows[name]
            span = "–" if row["low"] is None else f"{fmt(row['low'])}–{fmt(row['high'])}"
            flag = {"breakage": "**breakage**", "regression": "**regression**"}.get(row["flag"], "")
            lines.append(f"| {area} | {name} | {kind} | {fmt(row['value'])} | {fmt(row['baseline'])} | {span} "
                         f"| {flag} | {sparkline(row['recent'] + [row['value']])} |")
    return "\n".join(lines), rows


def flags(history, record, measures, series_key):
    rows, _count = compare(history, record, measures, series_key)
    return {name: row["flag"] for name, row in rows.items() if row["flag"]}
