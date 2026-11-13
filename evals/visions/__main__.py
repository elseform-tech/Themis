"""`python3 -m evals.visions run|impact|compare`: the Visions eval (benchmark audit, not a CI check).

    python3 -m evals.visions run --model MODEL --runs 3 --label main      # both arms, every task
    python3 -m evals.visions run --dry                                     # scripted provider, spends nothing
    python3 -m evals.visions impact --model MODEL --label main             # the labelled impact set
    python3 -m evals.visions compare --label main                          # Vision against goal mode (offline)

A round refuses to start when its caps are above the kit's (`evals/kit/spend.py`), when the model
is dearer than `--max-price` or not listed, or when a Themis Keychain entry is visible; each run
records its estimated `cost_usd`.
"""

import argparse
import datetime
import json
import os
import statistics
import subprocess
import sys
import tempfile

from evals.kit import app
from evals.kit import spend as spend_kit
from evals.kit import trend as kit_trend
from evals.kit.env import isolated_env, keychain_visible

from . import grade as grading
from . import scenarios

HERE = os.path.dirname(os.path.abspath(__file__))
HISTORY = os.path.join(HERE, "history.jsonl")
RESULTS = os.path.join(HERE, "results")
ROW_CLASSIFICATION = ("proxy: the share of labelled questions the Vision put in their decision row when asked "
                      "in its channel; not the decision it records on a real answer_question")


def commit():
    done = subprocess.run(["git", "rev-parse", "HEAD"], cwd=HERE, capture_output=True, text=True, check=False)
    return done.stdout.strip()


def record_of(trace, measures, options):
    date = trace.get("date") or datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")
    end = trace.get("end") or {}
    return {"id": f"{date}-{trace['scenario']}-{trace['arm']}-{trace.get('seed')}", "date": date,
            "commit": options.get("commit", ""), "label": options.get("label", ""),
            "kind": trace.get("kind", "task"), "arm": trace["arm"], "scenario": trace["scenario"],
            "seed": trace.get("seed"), "model": trace.get("model"), "end": end.get("reason"),
            "infrastructure": end.get("reason") in ("infrastructure", "crash"),
            "wall_minutes": round((trace.get("wall_s") or 0) / 60, 1), "measures": measures,
            "details": {"end_detail": (end.get("detail") or "")[:200],
                        "hidden": (trace.get("hidden") or {}).get("line", ""),
                        "chosen": trace.get("chosen"),
                        **({"row_classification": ROW_CLASSIFICATION} if trace.get("kind") == "impact" else {})}}


def above_caps(args):
    """Why the round's caps may not be used, or '' (checked before anything starts)."""
    caps = (("--max-run-cost", args.max_run_cost, spend_kit.MAX_RUN_COST),
            ("--max-round-cost", args.max_round_cost, spend_kit.MAX_ROUND_COST),
            ("--max-price", args.max_price, spend_kit.MAX_INPUT_PRICE))
    for flag, value, cap in caps:
        if value > cap:
            return f"{flag} {value} is above the kit's cap of {cap} (evals/kit/spend.py)"
    return ""


def prepare(args):
    """(options, price, provider) for the round, or (None, reason, None) when it may not start."""
    reason = above_caps(args)
    if reason:
        return None, reason, None
    options = {"timeout": args.timeout, "max_run_cost": args.max_run_cost, "autonomy": args.autonomy,
               "keep": args.keep, "commit": commit(), "label": args.label or ""}
    if args.dry:
        from .run import ScriptedProvider

        provider = ScriptedProvider()
        options.update(provider.options(), label=args.label or "dry")
        return options, spend_kit.Price(0.0, 0.0), provider
    catalog = spend_kit.load_catalog(args.catalog)
    info, reason = spend_kit.check_model(catalog, args.model, None, args.max_price)
    if info is None:
        return None, reason, None
    ok, reason = spend_kit.preflight(args.model, args.base_url)
    if not ok:
        return None, f"preflight: {reason}", None
    with tempfile.TemporaryDirectory(prefix="visions-keychain-") as scratch:
        if keychain_visible(isolated_env(scratch)):
            return None, "a Themis Keychain entry is visible from the scratch HOME", None
    options["model"] = args.model
    return options, (info.tiers[-1][1] if info.tiers else info.price), None


def cmd_run(args, jobs=None):
    from .run import Run

    options, price, provider = prepare(args)
    if options is None:
        print(f"refusing to run: {price}", file=sys.stderr)
        return 2
    binary = args.themis or app.build_app()
    # A dry run is a smoke check of the harness: its records stay out of the history.
    history = os.path.join(args.out, "history-dry.jsonl") if args.dry else args.history
    os.makedirs(args.out, exist_ok=True)
    round_spend = spend_kit.RoundSpend(price, args.max_round_cost, args.max_run_cost)
    records = []
    try:
        for scenario, seed, arm in jobs or arms(args):
            if not round_spend.can_start(args.max_run_cost):
                print(f"round cap: ${round_spend.committed():.2f} of ${args.max_round_cost:.2f}; skipping "
                      f"{scenario['name']} {arm} seed {seed}", file=sys.stderr)
                continue
            run = Run(binary, options, scenario, seed, arm, price)
            key = (scenario["name"], arm, seed)
            round_spend.start(key, run.meter, args.max_run_cost)
            trace = run.run()
            round_spend.finish(key, scenario["name"], trace.get("cost") or 0.0)
            with open(os.path.join(args.out, f"visions-{scenario['name']}-{arm}-{seed}.json"), "w",
                      encoding="utf-8") as handle:
                json.dump(trace, handle)
            record = record_of(trace, grading.grade(trace), options)
            kit_trend.append(history, record)
            records.append(record)
            print(f"{scenario['name']} {arm} seed {seed}: {record['end']} {json.dumps(record['measures'])}",
                  file=sys.stderr, flush=True)
    finally:
        if provider is not None:
            provider.close()
    print(compare_table(records, options["label"]))
    print(f"history: {history}", file=sys.stderr)
    return 0


def arms(args):
    names = args.scenarios or scenarios.names()
    seeds = list(range(1, args.runs + 1)) if args.runs else args.seeds
    for name in names:
        scenario = scenarios.load(name)
        for seed in seeds:
            for arm in scenario["arms"]:
                if arm in args.arms:
                    yield scenario, seed, arm


def cmd_impact(args):
    items = scenarios.impact_set()
    if args.limit:
        items = items[:args.limit]
    scenario = {"name": "impact", "arms": ["impact"], "verify": None, "message": "", "items": items}
    return cmd_run(args, jobs=[(scenario, seed, "impact") for seed in (args.seeds or [1])])


def compare_table(records, label):
    """Means per scenario and arm: the Vision against the person in goal mode. Runs stopped at a
    cap stay out of the `person_messages` mean (they nudge less because they end early) and are
    counted apart; `solved` is the share of runs whose hidden checks passed at the end."""
    rows = [r for r in records if r.get("label") == label and not r.get("infrastructure")]
    lines = [f"# Visions: `{label}` (benchmark audit)", ""]
    for name in sorted({r["scenario"] for r in rows}):
        groups = {arm: [r for r in rows if r["scenario"] == name and r["arm"] == arm]
                  for arm in ("vision", "goal", "impact")}
        groups = {arm: group for arm, group in groups.items() if group}
        lines += [f"## {name} ({', '.join(f'{arm} n = {len(g)}' for arm, g in groups.items())})", "",
                  "| Measure | " + " | ".join(groups) + " |", "|---" * (len(groups) + 1) + "|"]
        for measure in grading.MEASURES:
            cells = []
            for group in groups.values():
                if measure == "person_messages":
                    group = [r for r in group if r.get("end") not in grading.CAPPED]
                values = [r["measures"].get(measure) for r in group]
                values = [v for v in values if isinstance(v, (int, float))]
                cells.append(f"{statistics.mean(values):.3g}" if values else "–")
            if any(cell != "–" for cell in cells):
                lines.append(f"| {measure} | " + " | ".join(cells) + " |")
        capped = [str(sum(r.get("end") in grading.CAPPED for r in group)) for group in groups.values()]
        lines += ["| capped runs | " + " | ".join(capped) + " |", ""]
    return "\n".join(lines)


def cmd_compare(args):
    print(compare_table(kit_trend.load(args.history), args.label))
    return 0


def add_round_flags(parser):
    parser.add_argument("--model", default=spend_kit.DEFAULT_MODEL, help="the model both arms use")
    parser.add_argument("--seeds", default=[1], type=lambda text: [int(s) for s in text.split(",")])
    parser.add_argument("--timeout", type=int, default=1800, help="seconds before a run is capped")
    parser.add_argument("--max-run-cost", type=float, default=0.50, help="≈ dollars one run may spend")
    parser.add_argument("--max-round-cost", type=float, default=spend_kit.MAX_ROUND_COST)
    parser.add_argument("--max-price", type=float, default=spend_kit.MAX_INPUT_PRICE,
                        help="refuse models dearer than this per million input tokens")
    parser.add_argument("--autonomy", default="small", choices=("ask", "small", "most", "nearly_all"),
                        help="visions.autonomy for the run (default: the app's default, small)")
    parser.add_argument("--catalog", help="models.dev catalog file (default: fetched)")
    parser.add_argument("--base-url", help="provider base URL for the preflight (default: OpenCode Go)")
    parser.add_argument("--themis", help="a prebuilt themis CLI (default: build this checkout)")
    parser.add_argument("--label", help="a name for this arm of an A/B (a branch)")
    parser.add_argument("--dry", action="store_true",
                        help="a scripted local provider instead of a model: checks the harness, spends nothing")
    parser.add_argument("--history", default=HISTORY)
    parser.add_argument("--out", default=os.path.join(RESULTS, datetime.date.today().isoformat()))
    parser.add_argument("--keep", help="copy each run's scratch folders here")


def main(argv=None):
    parser = argparse.ArgumentParser(prog="python3 -m evals.visions")
    commands = parser.add_subparsers(dest="command", required=True)
    run = commands.add_parser("run", help="the scenarios live through the app (spends money)")
    add_round_flags(run)
    run.add_argument("--scenarios", type=lambda text: text.split(","), help=", ".join(scenarios.names()))
    run.add_argument("--arms", default=["vision", "goal"], type=lambda text: text.split(","))
    run.add_argument("--runs", type=int, help="runs per scenario and arm (seeds 1..N; overrides --seeds)")
    run.set_defaults(func=cmd_run)
    impact = commands.add_parser("impact", help="the labelled impact set through a Vision (spends money)")
    add_round_flags(impact)
    impact.add_argument("--limit", type=int, help="only the first N questions")
    impact.set_defaults(func=cmd_impact)
    compare = commands.add_parser("compare", help="means per scenario and arm from the history (offline)")
    compare.add_argument("--history", default=HISTORY)
    compare.add_argument("--label", default="")
    compare.set_defaults(func=cmd_compare)
    args = parser.parse_args(argv)
    for name in getattr(args, "scenarios", None) or []:
        if name not in scenarios.ALL:
            parser.error(f"unknown scenario {name!r}")
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
