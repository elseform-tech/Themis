"""CLI for the verification eval (a live benchmark audit, not an automated check).

    python3 -m evals.verify run --model M [--scenarios a,b] [--budgets 32000,64000,auto]
                                [--profiles none,mixed] [--seeds 1,2] [--themis BIN --label NAME]
    python3 -m evals.verify report TRACE.json [...]   # grade saved traces against the history
    python3 -m evals.verify trend [--model M]          # the latest run of every series
    python3 -m evals.verify ab --history H --a LABEL --b LABEL   # A against B, per budget

A series is (model, scenario, budget, profile): the budget and the pressure profile are trend and
A/B dimensions, never pooled.
"""

import argparse
import datetime
import json
import os
import random
import statistics
import subprocess
import sys
import tempfile

from evals.kit import app
from evals.kit import spend as spend_kit
from evals.kit import trend as kit_trend
from evals.kit.env import isolated_env, keychain_visible

from . import pressure, scenarios
from .grade import AREAS, MEASURES, grade, infrastructure

HERE = os.path.dirname(os.path.abspath(__file__))
HISTORY = os.path.join(HERE, "history.jsonl")
RESULTS = os.path.join(HERE, "results")
SEARXNG = os.path.join(app.ROOT, "crates", "desktop", "searxng-runtime")


def parse_budget(text):
    text = text.strip()
    if text == "auto":
        return "auto"
    value = int(text)
    if not 8000 <= value <= 1_000_000:
        raise argparse.ArgumentTypeError(f"budget {value} is outside 8000..1000000")
    return value


def series_key(record):
    return (record.get("model"), record.get("scenario"), str(record.get("budget")), record.get("profile"))


def commit():
    head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=app.ROOT, capture_output=True, text=True, check=False)
    return head.stdout.strip()


def record_of(trace, measures, details, options):
    now = datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat()
    key = f"{trace['scenario']}-{trace['budget']}-{trace['profile']}-{trace['seed']}"
    return {"id": f"{now}-{key}", "date": now, "commit": options.get("commit", ""), "label": options.get("label", ""),
            "model": trace["model"], "scenario": trace["scenario"], "kind": trace["kind"], "budget": trace["budget"],
            "profile": trace["profile"], "seed": trace["seed"], "infrastructure": infrastructure(trace),
            "measures": measures, "details": details}


def arms(names, budgets, profiles, seeds):
    """(scenario, budget, profile, seed), seed by seed, the rest in a shuffled order per seed."""
    out = []
    for seed in seeds:
        order = [(name, budget, profile) for name in names for budget in budgets for profile in profiles]
        random.Random(f"verify-order:{seed}").shuffle(order)
        out += [(name, budget, profile, seed) for name, budget, profile in order]
    return out


def report(history, records):
    out = ["# Verification eval (benchmark audit)", "",
           "A live trend eval, not an automated check: each run works on a generated Ledgerly project and is "
           "graded by hidden checks it never sees; each measure is compared with the recent runs of the same "
           "model, scenario, budget and pressure profile. A flag asks a person to look.", ""]
    for budget in sorted({str(r.get("budget")) for r in records}, key=lambda b: (b == "auto", b)):
        out += [f"## Budget {budget}", ""]
        for record in (r for r in records if str(r.get("budget")) == budget):
            details, measures = record.get("details") or {}, record.get("measures") or {}
            out += [f"### {record['scenario']} ({record['kind']}), pressure {record['profile']}, seed {record['seed']}"
                    + (f" — {record['label']}" if record.get("label") else ""), "",
                    f"End: **{details.get('end_state')}**"
                    + (f" ({details['end_detail']})" if details.get("end_detail") else "")
                    + f"; ≈ ${kit_trend.fmt(measures.get('cost_usd'))}, {kit_trend.fmt(measures.get('wall_minutes'))} min, "
                    f"{kit_trend.fmt(measures.get('tool_calls'))} tool calls, {kit_trend.fmt(measures.get('checkpoints'))} "
                    f"checkpoints. Hidden: `{details.get('hidden', '')}`; project tests: `{details.get('project_tests', '')}`", ""]
            if details.get("pressure"):
                out += ["Requests: " + ", ".join(f"{p['id']} {'ok' if p['ok'] else '**missed**'}"
                                                  for p in details["pressure"]), ""]
            if record.get("infrastructure"):
                out += ["Lost to infrastructure (provider): recorded, left out of the trend.", ""]
            if details.get("unclassified"):
                out += ["**Unclassified tool errors**: " + "; ".join(f"`{u['tool']}` {u['error']}"
                                                                     for u in details["unclassified"][:10]), ""]
            text, _rows = kit_trend.table(history, record, MEASURES, AREAS, series_key,
                                          f"model `{record.get('model')}`, {record.get('scenario')}, budget "
                                          f"{record.get('budget')}, pressure {record.get('profile')}")
            out += [text, "", "Final answer (end):", "", "> " + (details.get("answer") or "").replace("\n", "\n> "), ""]
    return "\n".join(out).rstrip() + "\n"


def ab_table(history, label_a, label_b):
    """Markdown: per budget, each measure's mean for A and for B over the runs both labels have."""
    out = [f"# A/B: `{label_a}` against `{label_b}`", "",
           "Means over every run of each label, per budget (and pressure profile); n is the run count.", ""]
    groups = {}
    for record in history:
        if record.get("label") in (label_a, label_b) and not record.get("infrastructure"):
            groups.setdefault((str(record.get("budget")), record.get("profile")), []).append(record)
    for (budget, profile), records in sorted(groups.items()):
        a = [r for r in records if r["label"] == label_a]
        b = [r for r in records if r["label"] == label_b]
        out += [f"## Budget {budget}, pressure {profile} (n = {len(a)} / {len(b)})", "",
                f"| Measure | {label_a} | {label_b} |", "| --- | --- | --- |"]
        for name, (_area, kind, _better) in MEASURES.items():
            values_a = [r["measures"].get(name) for r in a if r["measures"].get(name) is not None]
            values_b = [r["measures"].get(name) for r in b if r["measures"].get(name) is not None]
            if not values_a and not values_b:
                continue
            mean = lambda values: kit_trend.fmt(round(statistics.mean(values), 3)) if values else "–"  # noqa: E731
            out.append(f"| {name} | {mean(values_a)} | {mean(values_b)} |")
        out.append("")
    return "\n".join(out)


def cmd_run(args):
    from .run import VerifyRun  # imported here so `report`/`trend` need no app

    catalog = spend_kit.load_catalog(args.catalog)
    info, reason = spend_kit.check_model(catalog, args.model, None, args.max_price)
    if info is None:
        print(reason, file=sys.stderr)
        return 2
    ok, reason = spend_kit.preflight(args.model, args.base_url)
    if not ok:
        print(f"preflight: {reason}", file=sys.stderr)
        return 2
    with tempfile.TemporaryDirectory(prefix="verify-keychain-") as scratch:
        if keychain_visible(isolated_env(scratch)):
            print("refusing to run: a Themis Keychain entry is visible from the scratch HOME", file=sys.stderr)
            return 2
    binary = args.themis or app.build_app()
    searxng = args.searxng or (SEARXNG if os.path.isdir(SEARXNG) else None)
    if not searxng:
        print("note: no packed SearXNG (node scripts/bundle-searxng.mjs); web_search is off", file=sys.stderr)
    options = {"model": args.model, "timeout": args.timeout, "max_run_cost": args.max_run_cost,
               "keep": args.keep, "searxng": searxng, "commit": commit(), "label": args.label or ""}
    history = kit_trend.load(args.history)
    os.makedirs(args.out, exist_ok=True)
    records = []
    round_spend = None
    for name, budget, profile, seed in arms(args.scenarios, args.budgets, args.profiles, args.seeds):
        price = info.price_at(budget) if isinstance(budget, int) else (info.tiers[-1][1] if info.tiers else info.price)
        if round_spend is None:
            round_spend = spend_kit.RoundSpend(price, args.max_round_cost, args.max_run_cost)
        estimate = args.max_run_cost
        if not round_spend.can_start(estimate):
            print(f"round cap: ${round_spend.committed():.2f} of ${args.max_round_cost:.2f}; skipping "
                  f"{name} {budget} {profile} seed {seed}", file=sys.stderr)
            continue
        scenario = scenarios.load(name)
        run = VerifyRun(binary, options, scenario, seed, budget, profile, price)
        key = (name, budget, profile, seed)
        round_spend.start(key, run.meter, estimate)
        trace = run.run()
        round_spend.finish(key, name, trace.get("cost") or 0.0)
        tag = f"verify-{name}-{budget}-{profile}-{seed}"
        with open(os.path.join(args.out, f"{tag}.json"), "w", encoding="utf-8") as handle:
            json.dump(trace, handle)
        measures, details = grade(trace, scenario)
        record = record_of(trace, measures, details, options)
        history.append(record)
        kit_trend.append(args.history, record)
        records.append(record)
        print(f"{tag}: {trace['end']['reason']}, requirements {measures['requirements_met']}, "
              f"requests {measures['requests_completed']}, ≈ ${trace.get('cost') or 0:.3f}, "
              f"{trace['wall_s'] / 60:.1f} min", file=sys.stderr)
    text = report(history, records)
    report_path = os.path.join(args.out, "report.md")
    with open(report_path, "w", encoding="utf-8") as handle:
        handle.write(text)
    print(text)
    print(f"report: {report_path}; history: {args.history}", file=sys.stderr)
    return 0


def cmd_report(args):
    history = kit_trend.load(args.history)
    records = []
    for path in args.traces:
        with open(path, encoding="utf-8") as handle:
            trace = json.load(handle)
        measures, details = grade(trace, scenarios.load(trace["scenario"]))
        records.append(record_of(trace, measures, details, {"commit": args.commit or "", "label": args.label or ""}))
    print(report(history, records))
    return 0


def cmd_trend(args):
    history = kit_trend.load(args.history)
    latest = {}
    for record in history:
        if args.model and record.get("model") != args.model:
            continue
        latest[series_key(record)] = record
    if not latest:
        print("no runs in the history", file=sys.stderr)
        return 1
    print(report(history, list(latest.values())))
    return 0


def cmd_ab(args):
    print(ab_table(kit_trend.load(args.history), args.a, args.b))
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(prog="python3 -m evals.verify")
    commands = parser.add_subparsers(dest="command", required=True)
    run = commands.add_parser("run", help="run the scenarios live through the app (spends money)")
    run.add_argument("--model", default=spend_kit.DEFAULT_MODEL)
    run.add_argument("--scenarios", default=scenarios.names(), type=lambda text: text.split(","))
    run.add_argument("--budgets", default=[32000, 64000, "auto"],
                     type=lambda text: [parse_budget(b) for b in text.split(",")],
                     help="comma-separated context budgets (tokens or auto); default 32000,64000,auto")
    run.add_argument("--profiles", default=["none"], type=lambda text: text.split(","),
                     help=f"pressure profiles: {', '.join(pressure.PROFILES)} (default none)")
    run.add_argument("--seeds", default=[1], type=lambda text: [int(s) for s in text.split(",")])
    run.add_argument("--timeout", type=int, default=1800, help="seconds before a run is capped")
    run.add_argument("--max-run-cost", type=float, default=0.50)
    run.add_argument("--max-round-cost", type=float, default=spend_kit.MAX_ROUND_COST)
    run.add_argument("--max-price", type=float, default=spend_kit.MAX_INPUT_PRICE,
                     help="refuse models dearer than this per million input tokens")
    run.add_argument("--catalog", help="models.dev catalog file (default: fetched)")
    run.add_argument("--base-url", help="provider base URL for the preflight (default: OpenCode Go)")
    run.add_argument("--themis", help="a prebuilt themis CLI (default: build this checkout)")
    run.add_argument("--searxng", help="a packed SearXNG folder (default: crates/desktop/searxng-runtime if present)")
    run.add_argument("--label", help="a name for this arm (a branch) for the report and `ab`")
    run.add_argument("--history", default=HISTORY)
    run.add_argument("--out", default=os.path.join(RESULTS, datetime.date.today().isoformat()))
    run.add_argument("--keep", help="copy each run's scratch folders here")
    run.set_defaults(func=cmd_run)
    report_cmd = commands.add_parser("report", help="grade saved traces against the history (offline)")
    report_cmd.add_argument("traces", nargs="+")
    report_cmd.add_argument("--history", default=HISTORY)
    report_cmd.add_argument("--commit")
    report_cmd.add_argument("--label")
    report_cmd.set_defaults(func=cmd_report)
    trend_cmd = commands.add_parser("trend", help="the latest run of every series against its history (offline)")
    trend_cmd.add_argument("--history", default=HISTORY)
    trend_cmd.add_argument("--model")
    trend_cmd.set_defaults(func=cmd_trend)
    ab = commands.add_parser("ab", help="two labels' means per budget and profile (offline)")
    ab.add_argument("--history", default=HISTORY)
    ab.add_argument("--a", required=True)
    ab.add_argument("--b", required=True)
    ab.set_defaults(func=cmd_ab)
    args = parser.parse_args(argv)
    for profile in getattr(args, "profiles", []):
        if profile not in pressure.PROFILES:
            parser.error(f"unknown pressure profile {profile!r}")
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
