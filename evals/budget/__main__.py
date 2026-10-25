"""Budget eval: how the context budget changes success, cost and time on one generated task at
three sizes.

    python3 -m evals.budget run --sizes small,medium,large --budgets 64000,128000,200000,400000,1000000 \\
        --trials 3 --model M --via runner --jobs 2 --out FILE

Live-model benchmark audit; not an automated runtime check. Each run gets a freshly generated
project (one seed per trial, shared by every budget of that trial), three messages in one chat,
and hidden tests per stage run in a sandbox. Spend is controlled from models.dev prices: see
evals/budget/README.md.
"""

import argparse
import concurrent.futures
import datetime
import json
import os
import random
import re
import sys
import threading

from evals.tools.__main__ import ROOT

from . import execute, measures, spend
from .generate import SIZES, TARGET_TOKENS
from .grade import grade, messages

RESULTS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "results")
# Per-run limits by size: wall time for the whole chat, and model turns per message.
TIMEOUT = {"small": 2 * 3600, "medium": 4 * 3600, "large": 8 * 3600}
MAX_TURNS = {"small": 150, "medium": 400, "large": 1000}
NOTE = "Live-model benchmark audit; not an automated runtime check. Costs are estimates (≈)."


def app_budget_limit(root=ROOT):
    """The largest context budget the app's runtime.jsonc accepts, read from its schema."""
    try:
        with open(os.path.join(root, "crates/core/src/configuration.rs"), encoding="utf-8") as handle:
            found = re.search(r'"context_token_budget":\{[^}]*"maximum":(\d+)', handle.read())
        return int(found.group(1)) if found else 256_000
    except OSError:
        return 256_000


def schedule(sizes, budgets, trials, seed, window, via):
    """Arms in run order: trial by trial, each trial's (size, budget) pairs shuffled, so no budget
    runs as a block. Budgets are clamped to the model's window; clamped duplicates are dropped."""
    arms, dropped = [], []
    for trial in range(trials):
        pairs, seen = [], set()
        for size in sizes:
            for requested in budgets:
                budget = spend.clamp(requested, window)
                if (size, budget) in seen:
                    dropped.append(f"{size} {requested} (same as {budget} after clamping)")
                    continue
                seen.add((size, budget))
                pairs.append((size, budget, requested))
        random.Random(f"budget-order-{seed}-{trial}").shuffle(pairs)
        for size, budget, requested in pairs:
            arms.append({"name": f"{size}-b{budget}-t{trial + 1}", "size": size, "budget": budget,
                         "requested": requested, "trial": trial, "seed": seed + trial, "via": via})
    return arms, sorted(set(dropped))


def parse_list(text, kind=str):
    return [kind(item) for item in text.split(",") if item]


def run(args):
    catalog = spend.load_catalog(args.catalog)
    info = spend.model_info(catalog, args.model)
    if info is None:
        print(f"{args.model} is not in models.dev provider {spend.PROVIDER}; its price is unknown", file=sys.stderr)
        return 2
    dear = sorted({spend.clamp(b, info.window) for b in args.budgets
                   if info.price_at(spend.clamp(b, info.window)).input > args.max_price})
    if dear:
        print(f"{args.model} costs more than --max-price {args.max_price} $/M input tokens at budget "
              f"{', '.join(map(str, dear))} (${info.price_at(dear[-1]).input}/M)", file=sys.stderr)
        return 2
    limit = app_budget_limit()
    if args.via == "app" and any(spend.clamp(b, info.window) > limit for b in args.budgets):
        print(f"budgets above {limit} need --via runner: the app's settings stop at {limit}", file=sys.stderr)
        return 2
    arms, dropped = schedule(args.sizes, args.budgets, args.trials, args.seed, info.window, args.via)
    price = info.price
    print(f"model {args.model}: window {info.window or 'unknown'}, ≈ ${price.input}/M in, ${price.output}/M out"
          + (f", ${price.cache_read}/M cached" if price.cache_read is not None else "")
          + ("" if not price.free else " (free)"), flush=True)
    for size, tier in info.tiers:
        print(f"  above {size} tokens of context: ≈ ${tier.input}/M in, ${tier.output}/M out "
              "(a run whose budget is above it is priced at it throughout)", flush=True)
    for line in dropped:
        print(f"skipped arm: {line}", flush=True)
    if args.dry_run:
        for arm in arms:
            print(f"{arm['name']} seed {arm['seed']}", flush=True)
        return 0
    ok, reason = spend.preflight(args.model, args.base_url)
    if not ok:
        print(f"preflight: {reason}", file=sys.stderr)
        return 2
    if args.via == "app":
        binary = os.path.abspath(args.themis) if args.themis else execute.build_app(ROOT)

        def call(arm, meter, over):
            return execute.run_app(binary, arm, messages, grade, meter, over, options(arm))
    else:
        runner = [os.path.abspath(args.runner)] if args.runner else execute.build_runner(ROOT)

        def call(arm, meter, over):
            return execute.run_runner(runner, arm, messages, grade, meter, over, options(arm))

    def options(arm):
        return {"model": args.model, "base_url": args.base_url, "sandbox": args.sandbox, "keep": args.keep,
                "timeout": args.timeout or TIMEOUT[arm["size"]], "max_turns": args.max_turns or MAX_TURNS[arm["size"]]}

    os.makedirs(RESULTS, exist_ok=True)
    out = args.out or os.path.join(RESULTS, f"{datetime.date.today()}-{args.model}.json")
    budget = spend.RoundSpend(price, args.max_round_cost, args.max_run_cost)
    state = {"rows": [], "errors": [], "not_run": [], "aborted": None}
    lock = threading.Lock()

    def save():
        with lock:
            summary = measures.summarize(state["rows"])
            chosen, why = measures.ceiling(summary)
            result = {"suite": "budget", "model": args.model, "via": args.via, "trials": args.trials,
                      "created": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
                      "note": NOTE,
                      "price_per_million": {
                          "input": price.input, "output": price.output, "cache_read": price.cache_read,
                          "cache_write": price.cache_write,
                          "tiers": [[size, tier.input, tier.output] for size, tier in info.tiers]},
                      "window": info.window, "settings": {
                          "sizes": args.sizes, "budgets": args.budgets, "seed": args.seed,
                          "max_price": args.max_price, "max_round_cost": args.max_round_cost,
                          "max_run_cost": args.max_run_cost, "skipped_arms": dropped},
                      "spend": {"spent_usd": round(budget.spent, 4), "max_round_cost": args.max_round_cost},
                      "aborted": state["aborted"], "summary": summary,
                      "ceiling": {"large": chosen, "why": why,
                                  "only_large_matters": measures.only_large_matters(summary)},
                      "errors": state["errors"], "not_run": state["not_run"], "rows": state["rows"]}
            with open(out, "w", encoding="utf-8") as handle:
                json.dump(result, handle, indent=2, ensure_ascii=False)
                handle.write("\n")
            return result

    def one(arm, meter):
        try:
            outcome = call(arm, meter, budget.over_run_limit)
        except Exception as failure:  # noqa: BLE001 - one run must never abort the round
            return arm, None, f"harness: {type(failure).__name__}: {str(failure)[:160]}"
        if "infrastructure" in outcome:
            return arm, None, outcome["infrastructure"]
        if outcome.get("calls") == [] and outcome.get("error") and not outcome.get("stopped"):
            return arm, None, f"provider before any call: {outcome['error'][:160]}"
        return arm, outcome, None

    queue = list(arms)
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        running = {}
        while queue or running:
            while queue and len(running) < args.jobs and not state["aborted"]:
                arm = queue[0]
                arm_price = info.price_at(arm["budget"])
                estimate = budget.estimate(arm["size"], TARGET_TOKENS[arm["size"]], arm_price)
                if not budget.can_start(estimate):
                    break
                queue.pop(0)
                meter = spend.Meter(arm_price)
                budget.start(arm["name"], meter, estimate)
                running[pool.submit(one, arm, meter)] = (arm, meter)
            if not running:
                for arm in queue:
                    state["not_run"].append({"arm": arm["name"], "why": state["aborted"] or "round cost limit"})
                if queue and not state["aborted"]:
                    print(f"round cost limit: ≈ ${budget.committed():.2f} spent of ${args.max_round_cost:.2f}; "
                          f"{len(queue)} runs not started", flush=True)
                queue = []
                break
            done, _ = concurrent.futures.wait(running, return_when=concurrent.futures.FIRST_COMPLETED)
            for future in done:
                arm, meter = running.pop(future)
                arm, outcome, error = future.result()
                spent = meter.cost() if outcome is None else spend.cost(meter.price, outcome["usage"])
                budget.finish(arm["name"], arm["size"], spent)
                gone_text = error or (outcome or {}).get("error")
                if gone_text and spend.model_gone(gone_text):
                    still, reason = spend.preflight(args.model, args.base_url)
                    if not still:
                        state["aborted"] = f"model unavailable: {reason}"
                if error:
                    state["errors"].append({"arm": arm["name"], "error": error})
                    print(f"{arm['name']}: error {error}", flush=True)
                else:
                    row = measures.make_row(arm, outcome, meter.price)
                    state["rows"].append(row)
                    print(f"{arm['name']}: {'pass' if row['passed'] else row['failure']} "
                          f"(stages {row['stages_passed']}/3, desks {row['desks_passed']}/{row['desks']}, "
                          f"{'n/a' if row['compactions'] is None else row['compactions']} compactions, "
                          f"≈ ${row['cost_usd']:.3f})", flush=True)
                print(f"spend ≈ ${budget.spent:.3f} of ${args.max_round_cost:.2f}", flush=True)
                save()
            if state["aborted"]:
                for arm in queue:
                    state["not_run"].append({"arm": arm["name"], "why": state["aborted"]})
                queue = []
    result = save()
    print(markdown(result))
    print(out)
    return 0


def fmt(cell, key):
    value = cell.get(key)
    if not value:
        return "n/a"
    if key in ("passed", "cached_share", "reread_share"):
        return f"{value['mean']:.0%} ± {value['sd']:.0%}"
    if key == "cost_usd":
        return f"≈ {value['mean']:.3f} ± {value['sd']:.3f}"
    if key in ("input_tokens", "output_tokens", "reasoning_output_tokens"):
        if value["mean"] < 10_000:
            return f"{value['mean']:.0f} ± {value['sd']:.0f}"
        return f"{value['mean'] / 1000:.0f}k ± {value['sd'] / 1000:.0f}k"
    return f"{value['mean']:.1f} ± {value['sd']:.1f}"


def table(summary, label=None):
    head = (["Arm"] if label else []) + ["Size", "Budget", "Runs"] + [title for _, title in measures.MEASURES] + ["Failures"]
    lines = ["| " + " | ".join(head) + " |", "|" + " --- |" * len(head)]
    for size, cells in summary.items():
        for budget, cell in cells.items():
            failures = ", ".join(f"{k} {v}" for k, v in cell["failures"].items()) or "-"
            lines.append("| " + " | ".join(([label] if label else []) + [size, budget, str(cell["runs"])]
                                           + [fmt(cell, key) for key, _ in measures.MEASURES] + [failures]) + " |")
    return lines


def markdown(result):
    summary = result["summary"]
    chosen, why = measures.ceiling(summary)
    only = measures.only_large_matters(summary)
    lines = [f"# Budget eval: {result['model']}", "",
             f"Via: {result['via']}. Runs: {len(result['rows'])}; infrastructure errors (excluded): "
             f"{len(result['errors'])}; not run: {len(result.get('not_run', []))}. "
             f"Spend ≈ ${result['spend']['spent_usd']:.2f}. Benchmark audit, not an automated check. "
             "Values are mean ± sd.", ""]
    if result.get("aborted"):
        lines += [f"Round aborted: {result['aborted']}.", ""]
    lines += table(summary)
    lines += ["", f"Auto ceiling (large size): **{chosen if chosen else 'n/a'}** ({why}).",
              "Small and medium pass at every budget of 128k and above: "
              + {True: "yes", False: "no", None: "n/a"}[only] + "."]
    return "\n".join(lines) + "\n"


def compare(paths):
    lines = []
    for item in paths:
        label, _, path = item.rpartition("=")
        label = label or os.path.splitext(os.path.basename(path))[0]
        with open(path, encoding="utf-8") as handle:
            summary = measures.summarize(json.load(handle)["rows"])
        body = table(summary, label)
        lines += body if not lines else body[2:]
    return "\n".join(lines) + "\n"


def main(argv=None):
    parser = argparse.ArgumentParser(prog="python3 -m evals.budget")
    sub = parser.add_subparsers(dest="command", required=True)
    cmd = sub.add_parser("run")
    cmd.add_argument("--sizes", type=lambda t: parse_list(t), default=["small", "medium", "large"])
    cmd.add_argument("--budgets", type=lambda t: parse_list(t, int), default=[64_000, 128_000, 200_000, 400_000, 1_000_000])
    cmd.add_argument("--trials", type=int, default=3)
    cmd.add_argument("--seed", type=int, default=1, help="seed of the first trial; trial n uses seed + n - 1")
    cmd.add_argument("--model", required=True)
    cmd.add_argument("--via", choices=("app", "runner"), default="app")
    cmd.add_argument("--jobs", type=int, default=1)
    cmd.add_argument("--out")
    cmd.add_argument("--max-price", type=float, default=0.20, help="refuse a model above this $/M input tokens")
    cmd.add_argument("--max-round-cost", type=float, default=5.0, help="≈ $ for the whole round")
    cmd.add_argument("--max-run-cost", type=float, default=1.50, help="≈ $ after which a run is stopped")
    cmd.add_argument("--catalog", help="models.dev api.json (URL or file; default the live catalog)")
    cmd.add_argument("--base-url", help="provider base URL (a mock in tests)")
    cmd.add_argument("--runner", help="prebuilt tool_eval binary (--via runner)")
    cmd.add_argument("--themis", help="prebuilt themis CLI (--via app)")
    cmd.add_argument("--timeout", type=int, help="seconds per run (default by size)")
    cmd.add_argument("--max-turns", type=int, help="model turns per message (default by size)")
    cmd.add_argument("--sandbox", choices=("auto", "off"), default="auto")
    cmd.add_argument("--keep", help="save each run's folders under this folder")
    cmd.add_argument("--dry-run", action="store_true", help="print the schedule and prices, run nothing")
    show = sub.add_parser("report")
    show.add_argument("result")
    side = sub.add_parser("compare")
    side.add_argument("results", nargs="+", help="result files, each optionally labelled name=path")
    args = parser.parse_args(argv)
    if args.command == "report":
        with open(args.result, encoding="utf-8") as handle:
            print(markdown(json.load(handle)))
        return 0
    if args.command == "compare":
        print(compare(args.results))
        return 0
    unknown = [s for s in args.sizes if s not in SIZES]
    if unknown:
        parser.error(f"unknown sizes: {', '.join(unknown)}")
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
