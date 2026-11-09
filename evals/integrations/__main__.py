"""`python3 -m evals.integrations run|ab`: the integrations eval (benchmark audit, not a CI check).

    python3 -m evals.integrations run --model beam-free --seeds 1,2,3 --themis path/to/themis --label main
    python3 -m evals.integrations ab --a main --b branch

Only free models (a name ending in `-free`) are accepted: the eval spends nothing. A run that hits
a 429 stops the whole command, printing the Retry-After, instead of trying again.
"""

import argparse
import datetime
import json
import os
import sys

from evals.kit import app

from . import scenarios
from .run import IntegrationsRun

HERE = os.path.dirname(os.path.abspath(__file__))
HISTORY = os.path.join(HERE, "history.jsonl")


def cmd_run(args):
    if not args.model.endswith("-free"):
        sys.exit("the integrations eval runs free models only (a name ending in -free)")
    binary = args.themis or app.themis_binary()
    names = args.scenarios.split(",") if args.scenarios else sorted(scenarios.ALL)
    seeds = [int(seed) for seed in args.seeds.split(",")]
    os.makedirs(args.out, exist_ok=True)
    for name in names:
        for seed in seeds:
            trace = IntegrationsRun(binary, {"model": args.model, "timeout": args.timeout, "keep": args.keep},
                                    scenarios.load(name), seed).run()
            with open(os.path.join(args.out, f"integrations-{args.label}-{name}-{seed}.json"), "w",
                      encoding="utf-8") as handle:
                json.dump(trace, handle)
            if trace["end"]["reason"] == "rate_limited":
                sys.exit(f"{name} seed {seed}: stopped, {trace['end']['detail']}")
            record = {"label": args.label, "scenario": name, "seed": seed, "model": args.model,
                      "date": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
                      "end": trace["end"]["reason"], "wall_s": trace["wall_s"],
                      "measures": trace.get("measures", {}), "detail": trace.get("detail", "")}
            with open(args.history, "a", encoding="utf-8") as handle:
                handle.write(json.dumps(record) + "\n")
            print(f"{name} seed {seed}: {trace['end']['reason']} {record['measures']} {record['detail']}", flush=True)


KEYS = ("met", "false_claim", "failed_calls", "tool_calls", "old_tool_calls", "themis_calls",
        "failed_themis_calls", "nudges", "subagent_started", "subagent_themis")


def ab_table(history, label_a, label_b):
    """One row per scenario: sums of the store being right and of false claims, means of the rest."""
    records = [json.loads(line) for line in open(history, encoding="utf-8") if line.strip()]
    rows = ["| Scenario | n | store right | false claims | failed calls (mean) | tool calls (mean) | old tools (mean) | `themis` calls (mean) | wall s (mean) |",
            "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for name in sorted({r["scenario"] for r in records if r["label"] in (label_a, label_b)}):
        for label in (label_a, label_b):
            got = [r for r in records if r["label"] == label and r["scenario"] == name]
            if not got:
                continue

            def total(key):
                return sum(r["measures"].get(key, 0) for r in got)

            def mean(key):
                values = [r["wall_s"] if key == "wall_s" else r["measures"].get(key, 0) for r in got]
                return f"{sum(values) / len(values):.1f}"

            rows.append(f"| {name} · {label} | {len(got)} | {total('met')}/{len(got)} | {total('false_claim')} | "
                        f"{mean('failed_calls')} | {mean('tool_calls')} | {mean('old_tool_calls')} | "
                        f"{mean('themis_calls')} | {mean('wall_s')} |")
    extra = [r for r in records if r["label"] in (label_a, label_b) and "subagent_started" in r["measures"]]
    lines = [f"# A/B: `{label_a}` against `{label_b}` (benchmark audit)", "", *rows, ""]
    for label in (label_a, label_b):
        got = [r for r in extra if r["label"] == label]
        if got:
            lines.append(f"- {label}, diagnose-subagent: subagent started {sum(r['measures']['subagent_started'] for r in got)}/{len(got)}, "
                         f"subagent ran `themis` {sum(r['measures']['subagent_themis'] for r in got)}/{len(got)}")
    return "\n".join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(prog="evals.integrations")
    commands = parser.add_subparsers(dest="command", required=True)
    run = commands.add_parser("run", help="run the scenarios live through the app")
    run.add_argument("--model", required=True)
    run.add_argument("--scenarios", help=f"comma-separated, from {', '.join(sorted(scenarios.ALL))}")
    run.add_argument("--seeds", default="1")
    run.add_argument("--themis", help="a prebuilt themis CLI (default: this checkout's debug build)")
    run.add_argument("--label", default="local")
    run.add_argument("--timeout", type=int, default=600, help="seconds before a run is capped")
    run.add_argument("--history", default=HISTORY)
    run.add_argument("--out", default=os.path.join(HERE, "traces"))
    run.add_argument("--keep", help="copy each run's folders here (evidence)")
    ab = commands.add_parser("ab", help="two labels side by side per scenario (offline)")
    ab.add_argument("--a", required=True)
    ab.add_argument("--b", required=True)
    ab.add_argument("--history", default=HISTORY)
    args = parser.parse_args(argv)
    if args.command == "run":
        cmd_run(args)
    else:
        print(ab_table(args.history, args.a, args.b))


if __name__ == "__main__":
    main()
