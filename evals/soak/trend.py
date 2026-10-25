"""History and trend: each run appends one record; a run is compared with the recent runs of its
series (same model, budget, job and size).

- baseline: the median of the last 5 recorded runs of the series before this one, leaving out
  runs lost to infrastructure;
- a model measure is flagged when it is worse than every one of those 5 runs; with fewer than
  5, values are shown without flags;
- a harness measure is flagged at any occurrence (a count above 0), however short the history.

Nothing is "pass" or "fail": a flag asks a person to look.
"""

import json
import os
import statistics

from .grade import MEASURES

WINDOW = 5
SPARKS = "▁▂▃▄▅▆▇█"
AREAS = ("goal", "compaction", "memory", "tools", "health")


def series_key(record):
    return (record.get("model"), str(record.get("budget")), record.get("job"), record.get("size"))


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


def prior_runs(history, record):
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


def assess(name, value, previous):
    """{value, baseline, low, high, flag, recent} for one measure against earlier values."""
    _area, kind, better = MEASURES[name]
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


def compare(history, record):
    """Every measure of `record` assessed against its series' earlier runs."""
    earlier = prior_runs(history, record)
    rows = {}
    for name in MEASURES:
        previous = [(run.get("measures") or {}).get(name) for run in earlier]
        rows[name] = assess(name, (record.get("measures") or {}).get(name), previous)
    return rows, len(earlier)


def sparkline(values):
    values = [v for v in values if v is not None]
    if not values:
        return ""
    low, high = min(values), max(values)
    if high == low:
        return SPARKS[3] * len(values)
    return "".join(SPARKS[round((v - low) / (high - low) * (len(SPARKS) - 1))] for v in values)


def _fmt(value):
    if value is None:
        return "–"
    if isinstance(value, float):
        if value.is_integer():
            return str(int(value))
        return f"{value:.3g}" if abs(value) < 1000 else str(round(value))
    return str(value)


def table(history, record):
    rows, count = compare(history, record)
    lines = [f"Series: model `{record.get('model')}`, budget {record.get('budget')}, job {record.get('job')}, "
             f"{record.get('size')} items; {count} earlier run(s)"
             + ("" if count >= WINDOW else f" (fewer than {WINDOW}: model measures are not flagged yet)"), "",
             "| Area | Measure | Kind | This run | Baseline | Range | Flag | Recent |",
             "| --- | --- | --- | --- | --- | --- | --- | --- |"]
    for area in AREAS:
        for name, (measure_area, kind, _better) in MEASURES.items():
            if measure_area != area:
                continue
            row = rows[name]
            span = "–" if row["low"] is None else f"{_fmt(row['low'])}–{_fmt(row['high'])}"
            flag = {"breakage": "**breakage**", "regression": "**regression**"}.get(row["flag"], "")
            lines.append(f"| {area} | {name} | {kind} | {_fmt(row['value'])} | {_fmt(row['baseline'])} | {span} "
                         f"| {flag} | {sparkline(row['recent'] + [row['value']])} |")
    return "\n".join(lines), rows


def flags(history, record):
    rows, _count = compare(history, record)
    return {name: row["flag"] for name, row in rows.items() if row["flag"]}


def report(history, records):
    """Markdown for the given runs (one section per run, grouped per budget)."""
    out = ["# Goal soak (benchmark audit)", "",
           "A live trend eval, not an automated check: each measure is compared with the recent runs "
           "of the same model, budget, job and size. A flag asks a person to look.", ""]
    budgets = []
    for record in records:
        if str(record.get("budget")) not in budgets:
            budgets.append(str(record.get("budget")))
    for budget in budgets:
        out += [f"## Budget {budget}", ""]
        for record in (r for r in records if str(r.get("budget")) == budget):
            out += _run_section(history, record)
    return "\n".join(out).rstrip() + "\n"


def _run_section(history, record):
    details = record.get("details") or {}
    measures = record.get("measures") or {}
    out = [f"### Seed {record.get('seed')} — {record.get('date', '')} at `{(record.get('commit') or '')[:10]}`", "",
           f"End: **{details.get('end_state')}**" + (f" ({details['end_detail']})" if details.get("end_detail") else "")
           + f"; ≈ ${_fmt(measures.get('cost_usd'))}, {_fmt(measures.get('wall_minutes'))} min, "
           f"{_fmt(measures.get('requests'))} requests, cached share {_fmt(measures.get('cached_share'))}.", ""]
    if record.get("infrastructure"):
        out += ["Lost to infrastructure (provider): recorded, left out of the trend.", ""]
    unclassified = details.get("unclassified") or []
    if unclassified:
        out += ["**Unclassified tool errors** (sort them, then add a pattern to `tool_causes.py`):", ""]
        out += [f"- `{item['tool']}`: {item['error']}" for item in unclassified[:20]]
        out.append("")
    ours = details.get("ours") or []
    if ours:
        out += ["Tool errors classified as ours:", ""]
        out += [f"- `{item['tool']}` ({item['code']}): {item['error']}" for item in ours[:20]]
        out.append("")
    text, _rows = table(history, record)
    out += [text, ""]
    retention = details.get("retention") or []
    if retention:
        out += ["Per checkpoint (kept in the saved summary; rule violations in files last written after it):", "",
                "| Checkpoint | Goal | Owner rule | Retired rule | Format change | Share | Written | Violations |",
                "| --- | --- | --- | --- | --- | --- | --- | --- |"]
        segments = details.get("segments") or {}
        for row in retention:
            kept = row["kept"]
            segment = segments.get(str(row["checkpoint"]), {})

            def mark(name, kept=kept):
                return "–" if name not in kept else ("yes" if kept[name] else "**no**")
            out.append(f"| {row['checkpoint']} | {mark('goal')} | {mark('rule_owner')} | {mark('rule_retired')} "
                       f"| {mark('format_change')} | {row['share']:.2f} | {segment.get('written', 0)} "
                       f"| {segment.get('violations', 0)} |")
        out.append("")
    return out
