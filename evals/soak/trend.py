"""The soak's history and trend (see `evals.kit.trend`): a series is one model, budget, job
and size.
"""

from evals.kit import trend as kit_trend
from evals.kit.trend import WINDOW, append, load, sparkline  # noqa: F401 - the soak's API

from .grade import MEASURES

AREAS = ("goal", "compaction", "memory", "tools", "health")
_fmt = kit_trend.fmt


def series_key(record):
    return (record.get("model"), str(record.get("budget")), record.get("job"), record.get("size"))


def prior_runs(history, record):
    return kit_trend.prior_runs(history, record, series_key)


def assess(name, value, previous):
    return kit_trend.assess(MEASURES[name], value, previous)


def compare(history, record):
    return kit_trend.compare(history, record, MEASURES, series_key)


def table(history, record):
    text = (f"model `{record.get('model')}`, budget {record.get('budget')}, job {record.get('job')}, "
            f"{record.get('size')} items")
    return kit_trend.table(history, record, MEASURES, AREAS, series_key, text)


def flags(history, record):
    return kit_trend.flags(history, record, MEASURES, series_key)


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
