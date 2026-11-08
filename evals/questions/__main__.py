"""`python3 -m evals.questions run|ab`: the questions eval (benchmark audit, not a CI check).

    python3 -m evals.questions run --model beam-free --seeds 1,2,3 --themis path/to/themis --label main
    python3 -m evals.questions ab --a main --b my-branch

Only free models (a name ending in `-free`) are accepted: the eval spends nothing.
"""

import argparse
import datetime
import json
import os
import sys

from evals.kit import app

from . import scenarios
from .run import QuestionsRun

HERE = os.path.dirname(os.path.abspath(__file__))
HISTORY = os.path.join(HERE, "history.jsonl")


def cmd_run(args):
    if not args.model.endswith("-free"):
        sys.exit("the questions eval runs free models only (a name ending in -free)")
    binary = args.themis or app.themis_binary()
    names = args.scenarios.split(",") if args.scenarios else sorted(scenarios.ALL)
    seeds = [int(seed) for seed in args.seeds.split(",")]
    os.makedirs(args.out, exist_ok=True)
    for name in names:
        for seed in seeds:
            trace = QuestionsRun(binary, {"model": args.model, "timeout": args.timeout},
                                 scenarios.load(name), seed).run()
            with open(os.path.join(args.out, f"questions-{name}-{seed}.json"), "w", encoding="utf-8") as handle:
                json.dump(trace, handle)
            record = {"label": args.label, "scenario": name, "seed": seed, "model": args.model,
                      "date": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
                      "end": trace["end"]["reason"], "wall_s": trace["wall_s"],
                      "measures": trace.get("measures", {}), "detail": trace.get("detail", "")}
            with open(args.history, "a", encoding="utf-8") as handle:
                handle.write(json.dumps(record) + "\n")
            print(f"{name} seed {seed}: {trace['end']['reason']} {record['measures']} {record['detail']}", flush=True)


def ab_table(history, label_a, label_b):
    records = [json.loads(line) for line in open(history, encoding="utf-8") if line.strip()]
    lines = [f"# A/B: `{label_a}` against `{label_b}` (benchmark audit)", ""]
    for name in sorted({r["scenario"] for r in records if r["label"] in (label_a, label_b)}):
        a = [r for r in records if r["label"] == label_a and r["scenario"] == name]
        b = [r for r in records if r["label"] == label_b and r["scenario"] == name]
        keys = sorted({k for r in a + b for k in r["measures"]} | {"wall_s"})
        lines += [f"## {name} (n = {len(a)} / {len(b)})", "", f"| Measure | {label_a} | {label_b} |", "| --- | --- | --- |"]
        for key in keys:
            def mean(rows):
                values = [r["wall_s"] if key == "wall_s" else r["measures"].get(key) for r in rows]
                values = [v for v in values if isinstance(v, (int, float))]
                return f"{sum(values) / len(values):.3g}" if values else "-"
            lines.append(f"| {key} | {mean(a)} | {mean(b)} |")
        lines.append("")
    return "\n".join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(prog="evals.questions")
    commands = parser.add_subparsers(dest="command", required=True)
    run = commands.add_parser("run", help="run the scenarios live through the app")
    run.add_argument("--model", required=True)
    run.add_argument("--scenarios", help=f"comma-separated, from {', '.join(sorted(scenarios.ALL))}")
    run.add_argument("--seeds", default="1")
    run.add_argument("--themis", help="a prebuilt themis CLI (default: build this checkout)")
    run.add_argument("--label", default="local")
    run.add_argument("--timeout", type=int, default=1200, help="seconds before a run is capped")
    run.add_argument("--history", default=HISTORY)
    run.add_argument("--out", default=os.path.join(HERE, "traces"))
    ab = commands.add_parser("ab", help="two labels' means per scenario (offline)")
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
