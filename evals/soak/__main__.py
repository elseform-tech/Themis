"""CLI for the goal soak (a live benchmark audit, not an automated check).

    python3 -m evals.soak run --model M [--budgets 24000,64000,auto] [--seeds 1,2,3]
    python3 -m evals.soak report TRACE.json [...]   # grade saved traces against the history
    python3 -m evals.soak trend [--model M]          # the latest run of every series
"""

import argparse
import datetime
import json
import os
import random
import subprocess
import sys

from evals.kit import app
from evals.kit import spend as spend_kit

from . import trend
from .generate import DEFAULT_FILLER_CHARS, DEFAULT_ITEMS
from .grade import grade, infrastructure

HERE = os.path.dirname(os.path.abspath(__file__))
HISTORY = os.path.join(HERE, "history.jsonl")
RESULTS = os.path.join(HERE, "results")


def parse_budget(text):
    text = text.strip()
    if text == "auto":
        return "auto"
    value = int(text)
    if not 2000 <= value <= 1_000_000:
        raise argparse.ArgumentTypeError(f"budget {value} is outside 2000..1000000")
    return value


def commit():
    head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=app.ROOT, capture_output=True, text=True, check=False)
    return head.stdout.strip()


def record_of(trace, measures, details, options):
    spec = trace["spec"]
    now = datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat()
    return {"id": f"{now}-{trace['budget']}-{trace['seed']}", "date": now, "commit": options.get("commit", ""),
            "model": trace["model"], "budget": trace["budget"], "seed": trace["seed"], "job": spec["job"],
            "size": spec["size"], "filler_chars": spec.get("filler_chars"),
            "infrastructure": infrastructure(trace), "measures": measures, "details": details}


def arms(budgets, seeds):
    """(budget, seed) pairs, seed by seed, budgets in a shuffled order per seed."""
    pairs = []
    for seed in seeds:
        order = list(budgets)
        random.Random(f"soak-order:{seed}").shuffle(order)
        pairs += [(budget, seed) for budget in order]
    return pairs


def cmd_run(args):
    from .run import Soak  # imported here so `report`/`trend` need no app

    catalog = spend_kit.load_catalog(args.catalog)
    price = None
    for budget in args.budgets:
        info, reason = spend_kit.check_model(catalog, args.model, budget, args.max_price)
        if info is None:
            print(reason, file=sys.stderr)
            return 2
        price = info.price if price is None else price
    ok, reason = spend_kit.preflight(args.model, args.base_url)
    if not ok:
        print(f"preflight: {reason}", file=sys.stderr)
        return 2
    binary = args.themis or app.build_app()
    options = {"model": args.model, "items": args.items, "filler_chars": args.filler_chars,
               "max_turns": args.max_turns, "timeout": args.timeout, "idle_timeout": args.idle_timeout,
               "poll_s": 5, "grace_s": 30, "max_run_cost": args.max_run_cost, "keep": args.keep,
               "commit": commit()}
    round_spend = spend_kit.RoundSpend(price, args.max_round_cost, args.max_run_cost)
    history = trend.load(args.history)
    os.makedirs(args.out, exist_ok=True)
    records = []
    for budget, seed in arms(args.budgets, args.seeds):
        info, _reason = spend_kit.check_model(catalog, args.model, budget, args.max_price)
        estimate = round_spend.estimate(str(budget))
        if not round_spend.can_start(estimate):
            print(f"round cap: ${round_spend.committed():.2f} spent of ${args.max_round_cost:.2f}; "
                  f"skipping budget {budget} seed {seed}", file=sys.stderr)
            continue
        for attempt in (1, 2):
            run_price = info.price_at(budget) if isinstance(budget, int) else (info.tiers[-1][1] if info.tiers else info.price)
            soak = Soak(binary, options, budget, seed, run_price)
            round_spend.start((budget, seed, attempt), soak.meter, estimate)
            trace = soak.run()
            round_spend.finish((budget, seed, attempt), str(budget), trace.get("cost") or 0.0)
            name = f"soak-{budget}-{seed}-{attempt}"
            with open(os.path.join(args.out, f"{name}.json"), "w", encoding="utf-8") as handle:
                json.dump(trace, handle)
            lost = infrastructure(trace)
            print(f"{name}: {trace['end']['reason']}{' (infrastructure)' if lost else ''}, "
                  f"≈ ${trace.get('cost') or 0:.3f}, {trace['wall_s'] / 60:.1f} min", file=sys.stderr)
            # Lost to the provider: rerun once, then record it as infrastructure.
            if lost and attempt == 1 and round_spend.can_start(estimate):
                continue
            measures, details = grade(trace, trace["spec"])
            record = record_of(trace, measures, details, options)
            history.append(record)
            trend.append(args.history, record)
            records.append(record)
            break
    text = trend.report(history, records)
    report_path = os.path.join(args.out, "report.md")
    with open(report_path, "w", encoding="utf-8") as handle:
        handle.write(text)
    print(text)
    print(f"report: {report_path}; history: {args.history}", file=sys.stderr)
    return 0


def cmd_report(args):
    history = trend.load(args.history)
    records = []
    for path in args.traces:
        with open(path, encoding="utf-8") as handle:
            trace = json.load(handle)
        measures, details = grade(trace, trace["spec"])
        records.append(record_of(trace, measures, details, {"commit": args.commit or ""}))
    print(trend.report(history, records))
    return 0


def cmd_trend(args):
    history = trend.load(args.history)
    latest = {}
    for record in history:
        if args.model and record.get("model") != args.model:
            continue
        latest[trend.series_key(record)] = record
    if not latest:
        print("no runs in the history", file=sys.stderr)
        return 1
    print(trend.report(history, list(latest.values())))
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(prog="python3 -m evals.soak")
    commands = parser.add_subparsers(dest="command", required=True)
    run = commands.add_parser("run", help="run the soak live through the app (spends money)")
    run.add_argument("--model", default=spend_kit.DEFAULT_MODEL)
    run.add_argument("--budgets", default=[24000], type=lambda text: [parse_budget(b) for b in text.split(",")],
                     help="comma-separated context budgets (tokens or auto); default 24000")
    run.add_argument("--seeds", default=[1], type=lambda text: [int(s) for s in text.split(",")])
    run.add_argument("--items", type=int, default=DEFAULT_ITEMS)
    run.add_argument("--filler-chars", type=int, default=DEFAULT_FILLER_CHARS)
    run.add_argument("--max-turns", type=int, default=80, help="goal turns before the run is capped")
    run.add_argument("--timeout", type=int, default=3600, help="seconds before the run is capped")
    run.add_argument("--idle-timeout", type=int, default=300,
                     help="seconds with the goal running and no run going before it counts as not continuing")
    run.add_argument("--max-run-cost", type=float, default=spend_kit.MAX_RUN_COST)
    run.add_argument("--max-round-cost", type=float, default=spend_kit.MAX_ROUND_COST)
    run.add_argument("--max-price", type=float, default=spend_kit.MAX_INPUT_PRICE,
                     help="refuse models dearer than this per million input tokens")
    run.add_argument("--catalog", help="models.dev catalog file (default: fetched)")
    run.add_argument("--base-url", help="provider base URL for the preflight (default: OpenCode Go)")
    run.add_argument("--themis", help="a prebuilt themis CLI (default: build this checkout)")
    run.add_argument("--history", default=HISTORY)
    run.add_argument("--out", default=os.path.join(RESULTS, datetime.date.today().isoformat()))
    run.add_argument("--keep", help="copy each run's scratch folders here")
    run.set_defaults(func=cmd_run)
    report = commands.add_parser("report", help="grade saved traces against the history (offline)")
    report.add_argument("traces", nargs="+")
    report.add_argument("--history", default=HISTORY)
    report.add_argument("--commit")
    report.set_defaults(func=cmd_report)
    trend_cmd = commands.add_parser("trend", help="the latest run of every series against its history (offline)")
    trend_cmd.add_argument("--history", default=HISTORY)
    trend_cmd.add_argument("--model")
    trend_cmd.set_defaults(func=cmd_trend)
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
