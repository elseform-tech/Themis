"""Long-horizon eval: big jobs that cross several context checkpoints, with and without a plan.

    python3 -m evals.horizon run --trials 3 --model longcat-2.5-preview-free

Live-model benchmark audit; not an automated runtime check. Uses the tool-eval runner
(`crates/core/examples/tool_eval.rs`) and its isolation (scratch project, scratch HOME,
`sandbox-exec` on macOS). At every checkpoint it compares the plan the task ledger carries
with the latest plan the agent saved.
"""

import argparse
import collections
import concurrent.futures
import datetime
import json
import os
import sys

from evals.tools.__main__ import ROOT, _mean, cargo_build, fmt, run_task

from .tasks import TASKS

RESULTS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "results")


def _steps(items):
    if not isinstance(items, list):
        return None
    return [(str(item.get("step", "")).strip(), item.get("status") or "pending")
            for item in items if isinstance(item, dict)]


def plan_persistence(row):
    """Per checkpoint: `kept` (carries the latest saved plan), `stale` (an older one), `dropped`
    (none although one was saved), `changed` (something else), or `no_plan_yet`."""
    plans = row.get("plans", [])
    verdicts = []
    for checkpoint in row.get("checkpoints", []):
        saved = [_steps(p) for p in plans[:checkpoint["plan_calls_before"]] if p is not None]
        carried = _steps(checkpoint.get("plan"))
        if not saved:
            verdicts.append("no_plan_yet")
        elif carried is None:
            verdicts.append("dropped")
        elif carried == saved[-1]:
            verdicts.append("kept")
        elif carried in saved:
            verdicts.append("stale")
        else:
            verdicts.append("changed")
    return verdicts


def summarize(rows):
    by_task = collections.defaultdict(list)
    for row in rows:
        by_task[row["task"]].append(row)
    tasks = {}
    for name, runs in sorted(by_task.items()):
        verdicts = [v for r in runs for v in plan_persistence(r)]
        counted = [v for v in verdicts if v != "no_plan_yet"]
        extra = lambda key: _mean([r["extra"].get(key) for r in runs if key in r.get("extra", {})])  # noqa: E731
        tasks[name] = {
            "arm": runs[0].get("arm"), "runs": len(runs), "pass_rate": _mean([r["passed"] for r in runs]),
            "mean_compactions": _mean([r["compactions"] for r in runs]),
            "plan_used_rate": _mean([bool(r.get("plans")) for r in runs]),
            "plan_kept_rate": _mean([v == "kept" for v in counted]),
            "checkpoints_with_plan": len(counted),
            "plan_verdicts": dict(collections.Counter(verdicts)),
            "early_detail_rate": extra("early_detail"), "count_recalled_rate": extra("count_recalled"),
            "mean_rereads": extra("rereads"), "mean_rule_kept_files": extra("rule_kept_files"),
            "mean_calls": _mean([len(r["calls"]) for r in runs]),
            "mean_input_tokens": _mean([r.get("input_tokens") for r in runs]),
            "failures": [r["why"] for r in runs if not r["passed"]][:3],
        }
    return {"runs": len(rows), "task_pass_rate": _mean([r["passed"] for r in rows]), "tasks": tasks}


def markdown(summary):
    s = summary["summary"]
    lines = [f"# Long horizon: {summary.get('model')}", "",
             f"Runs: {s['runs']}; infrastructure errors (excluded): {len(summary['errors'])}. "
             "Benchmark audit, not an automated check.", "",
             "| Task | Pass | Compactions | Plan used | Plan kept at checkpoints | Early detail recalled | "
             "Count recalled | Re-reads | Calls | Input tokens | First failure |",
             "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for name, row in s["tasks"].items():
        kept = f"{fmt(row['plan_kept_rate'])} of {row['checkpoints_with_plan']}" if row["checkpoints_with_plan"] else "-"
        lines.append(f"| {name} | {fmt(row['pass_rate'])} | {fmt(row['mean_compactions'], False)} | "
                     f"{fmt(row['plan_used_rate'])} | {kept} | {fmt(row['early_detail_rate'])} | "
                     f"{fmt(row['count_recalled_rate'])} | {fmt(row['mean_rereads'], False)} | "
                     f"{fmt(row['mean_calls'], False)} | {fmt(row['mean_input_tokens'], False)} | "
                     f"{(row['failures'] or ['-'])[0][:60]} |")
    return "\n".join(lines) + "\n"


def run(args):
    runner = [os.path.abspath(args.runner)] if args.runner else cargo_build(
        ROOT, os.path.join(ROOT, os.environ.get("CARGO_TARGET_DIR", "target")))
    tasks = [t for t in TASKS if not args.task or t["name"] in args.task or t["job"] in args.task]
    jobs = [(trial, task) for trial in range(args.trials) for task in tasks]
    rows, errors = [], []
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        futures = {pool.submit(run_task, runner, task, args): (trial, task) for trial, task in jobs}
        for future in concurrent.futures.as_completed(futures):
            trial, task = futures[future]
            row, error = future.result()
            if row and row.get("error_text", "").startswith("LLM request failed"):
                # The provider failed mid-run (truncated response, rate limit): infrastructure,
                # not a result. Kept with its measures for inspection, excluded from rates.
                error = f"provider failed mid-run after {len(row['calls'])} calls: {row['error_text'][:120]}"
                row = dict(row, plan_persistence=plan_persistence(row))
            if error:
                errors.append({"trial": trial, "task": task["name"], "error": error,
                               **({"row": row} if row else {})})
                print(f"trial {trial + 1} {task['name']}: error {error}", flush=True)
            else:
                rows.append(dict(row, trial=trial, arm=task["arm"], plan_persistence=plan_persistence(row)))
                print(f"trial {trial + 1} {task['name']}: {'pass' if row['passed'] else 'fail'} "
                      f"({row['compactions']} compactions, plan {plan_persistence(row)})", flush=True)
    summary = {"suite": "horizon", "model": args.model, "trials": args.trials,
               "created": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
               "note": "Live-model benchmark audit; not an automated runtime check.",
               "summary": summarize(rows), "errors": errors, "rows": rows}
    os.makedirs(RESULTS, exist_ok=True)
    out = args.out or os.path.join(RESULTS, f"{datetime.date.today()}-{args.model or 'mock'}.json")
    with open(out, "w", encoding="utf-8") as handle:
        json.dump(summary, handle, indent=2, ensure_ascii=False)
        handle.write("\n")
    print(markdown(summary))
    print(out)
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(prog="python3 -m evals.horizon")
    sub = parser.add_subparsers(dest="command", required=True)
    cmd = sub.add_parser("run")
    cmd.add_argument("--trials", type=int, default=3)
    cmd.add_argument("--model", default="longcat-2.5-preview-free")
    cmd.add_argument("--task", action="append", help="task or job name (repeatable)")
    cmd.add_argument("--jobs", type=int, default=2)
    cmd.add_argument("--max-turns", type=int, default=90)
    cmd.add_argument("--timeout", type=int, default=2400)
    cmd.add_argument("--base-url")
    cmd.add_argument("--runner", help="prebuilt tool_eval binary")
    cmd.add_argument("--sandbox", choices=("auto", "off"), default="auto")
    cmd.add_argument("--out")
    show = sub.add_parser("report")
    show.add_argument("result")
    args = parser.parse_args(argv)
    if args.command == "report":
        with open(args.result, encoding="utf-8") as handle:
            print(markdown(json.load(handle)))
        return 0
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
