"""CLI for the verification eval (a live benchmark audit, not an automated check).

    python3 -m evals.verify run --model M [--tasks bug-discount,web-clear] [--reps 2]
    python3 -m evals.verify report TRACE.json [...]   # grade saved traces against the history
    python3 -m evals.verify trend [--model M]          # the latest run of every series
"""

import argparse
import datetime
import json
import os
import subprocess
import sys
import tempfile

from evals.kit import app
from evals.kit import spend as spend_kit
from evals.kit import trend as kit_trend
from evals.kit.env import isolated_env, keychain_visible

from . import tasks
from .grade import AREAS, MEASURES, grade, infrastructure

HERE = os.path.dirname(os.path.abspath(__file__))
HISTORY = os.path.join(HERE, "history.jsonl")
RESULTS = os.path.join(HERE, "results")
SEARXNG = os.path.join(app.ROOT, "crates", "desktop", "searxng-runtime")


def series_key(record):
    return (record.get("model"), record.get("task"))


def commit():
    head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=app.ROOT, capture_output=True, text=True, check=False)
    return head.stdout.strip()


def record_of(trace, measures, details, options):
    now = datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat()
    return {"id": f"{now}-{trace['task']}-{trace.get('rep')}", "date": now, "commit": options.get("commit", ""),
            "label": options.get("label", ""), "model": trace["model"], "task": trace["task"],
            "kind": trace["kind"], "rep": trace.get("rep"), "infrastructure": infrastructure(trace),
            "measures": measures, "details": details}


def report(history, records):
    out = ["# Verification eval (benchmark audit)", "",
           "A live trend eval, not an automated check: each run is graded by hidden checks the agent "
           "never sees and compared with the recent runs of the same model and task. A flag asks a "
           "person to look.", ""]
    for record in records:
        details = record.get("details") or {}
        measures = record.get("measures") or {}
        out += [f"## {record['task']} ({record['kind']}) — rep {record.get('rep')}"
                + (f", {record['label']}" if record.get("label") else ""), "",
                f"End: **{details.get('end_state')}**" + (f" ({details['end_detail']})" if details.get("end_detail") else "")
                + f"; ≈ ${kit_trend.fmt(measures.get('cost_usd'))}, {kit_trend.fmt(measures.get('wall_minutes'))} min, "
                f"{kit_trend.fmt(measures.get('tool_calls'))} tool calls. Hidden check: `{details.get('hidden', '')}`", ""]
        if record.get("infrastructure"):
            out += ["Lost to infrastructure (provider): recorded, left out of the trend.", ""]
        text, _rows = kit_trend.table(history, record, MEASURES, AREAS, series_key,
                                      f"model `{record.get('model')}`, task {record.get('task')}")
        out += [text, "", "Final answer (end):", "", "> " + (details.get("answer") or "").replace("\n", "\n> "), ""]
    return "\n".join(out).rstrip() + "\n"


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
    # The model's dearest context tier: a run's window is not capped here.
    price = info.tiers[-1][1] if info.tiers else info.price
    round_spend = spend_kit.RoundSpend(price, args.max_round_cost, args.max_run_cost)
    history = kit_trend.load(args.history)
    os.makedirs(args.out, exist_ok=True)
    records = []
    for rep in range(1, args.reps + 1):
        for name in args.tasks:
            task = tasks.load(name)
            estimate = args.max_run_cost
            if not round_spend.can_start(estimate):
                print(f"round cap: ${round_spend.committed():.2f} of ${args.max_round_cost:.2f}; skipping {name} rep {rep}",
                      file=sys.stderr)
                continue
            run = VerifyRun(binary, options, task, rep, price)
            round_spend.start((name, rep), run.meter, estimate)
            trace = run.run()
            round_spend.finish((name, rep), name, trace.get("cost") or 0.0)
            with open(os.path.join(args.out, f"verify-{name}-{rep}.json"), "w", encoding="utf-8") as handle:
                json.dump(trace, handle)
            measures, details = grade(trace, task)
            record = record_of(trace, measures, details, options)
            history.append(record)
            kit_trend.append(args.history, record)
            records.append(record)
            print(f"{name} rep {rep}: {trace['end']['reason']}, correct {measures['correct']}, "
                  f"≈ ${trace.get('cost') or 0:.3f}, {trace['wall_s'] / 60:.1f} min", file=sys.stderr)
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
        measures, details = grade(trace, tasks.load(trace["task"]))
        records.append(record_of(trace, measures, details, {"commit": args.commit or ""}))
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


def main(argv=None):
    parser = argparse.ArgumentParser(prog="python3 -m evals.verify")
    commands = parser.add_subparsers(dest="command", required=True)
    run = commands.add_parser("run", help="run the tasks live through the app (spends money)")
    run.add_argument("--model", default=spend_kit.DEFAULT_MODEL)
    run.add_argument("--tasks", default=tasks.names(), type=lambda text: text.split(","))
    run.add_argument("--reps", type=int, default=1)
    run.add_argument("--timeout", type=int, default=1200, help="seconds before a run is capped")
    run.add_argument("--max-run-cost", type=float, default=0.30)
    run.add_argument("--max-round-cost", type=float, default=spend_kit.MAX_ROUND_COST)
    run.add_argument("--max-price", type=float, default=spend_kit.MAX_INPUT_PRICE,
                     help="refuse models dearer than this per million input tokens")
    run.add_argument("--catalog", help="models.dev catalog file (default: fetched)")
    run.add_argument("--base-url", help="provider base URL for the preflight (default: OpenCode Go)")
    run.add_argument("--themis", help="a prebuilt themis CLI (default: build this checkout)")
    run.add_argument("--searxng", help="a packed SearXNG folder (default: crates/desktop/searxng-runtime if present)")
    run.add_argument("--label", help="a name for this arm in the report, such as a branch")
    run.add_argument("--history", default=HISTORY)
    run.add_argument("--out", default=os.path.join(RESULTS, datetime.date.today().isoformat()))
    run.add_argument("--keep", help="copy each run's scratch folders here")
    run.set_defaults(func=cmd_run)
    report_cmd = commands.add_parser("report", help="grade saved traces against the history (offline)")
    report_cmd.add_argument("traces", nargs="+")
    report_cmd.add_argument("--history", default=HISTORY)
    report_cmd.add_argument("--commit")
    report_cmd.set_defaults(func=cmd_report)
    trend_cmd = commands.add_parser("trend", help="the latest run of every series against its history (offline)")
    trend_cmd.add_argument("--history", default=HISTORY)
    trend_cmd.add_argument("--model")
    trend_cmd.set_defaults(func=cmd_trend)
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
