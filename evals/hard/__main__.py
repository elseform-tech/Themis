"""Hard long-running tasks: a three.js night city and a spreadsheet engine.

    python3 -m evals.hard run --model mimo-v2.6-flash --memory both --trials 1

Live-model benchmark audit; not an automated runtime check. Each run gets a fresh scratch project
and HOME and, on macOS, `sandbox-exec` around the runner. The three.js grader measures the scene
in headless Chrome through its own page; the spreadsheet grader runs hidden tests in a sandbox.
"""

import argparse
import collections
import concurrent.futures
import datetime
import json
import os
import subprocess
import sys
import time

from evals.tools.__main__ import ROOT, _mean, cargo_build, fmt, run_task_retrying

from . import codebase, followup, moves, sheet, sheet_long, threejs
from .app import run_app_task, themis_binary

RESULTS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "results")
# The app's defaults, and a small budget that forces many checkpoints mid-task.
MEMORY = {"normal": {"budget": 200_000, "recent_messages": 20},
          "squeezed": {"budget": 32_000, "recent_messages": 8}}
JOBS = {"threejs": threejs, "sheet": sheet, "sheet-long": sheet_long, "codebase": codebase,
        "followup-sum": followup.SUM, "followup-tests": followup.TESTS, "followup-read": followup.READ}


def make_task(job, memory):
    module = JOBS[job]
    return {"name": f"{job}-{memory}", "tool": job, "job": job, "memory": memory,
            "setup": module.setup, "check": module.check, "prompt": module.PROMPT,
            "followups": getattr(module, "FOLLOWUPS", []),
            "evidence": ".themis/context/eval-thread",
            "timeout": getattr(module, "TIMEOUT", 5400),
            **({"measure": module.measure} if hasattr(module, "measure") else {}),
            # YOLO-style approvals as a developer would use them; the OS sandbox still applies.
            "allow_all": True, **memory_settings(memory)}


def memory_settings(memory):
    """A named memory preset, or `b<tokens>`: that budget with the app's default recent messages."""
    if memory.startswith("b") and memory[1:].isdigit():
        return {"budget": int(memory[1:]), "recent_messages": MEMORY["normal"]["recent_messages"]}
    return MEMORY[memory]


def summarize(rows):
    by_task = collections.defaultdict(list)
    for row in rows:
        by_task[row["task"]].append(row)
    return {"runs": len(rows), "pass_rate": _mean([r["passed"] for r in rows]), "tasks": {
        name: {"runs": len(runs), "pass_rate": _mean([r["passed"] for r in runs]),
               "mean_calls": _mean([len(r["calls"]) for r in runs]),
               # Moves a finished task took: fewer is better at the same pass rate.
               "mean_calls_per_pass": _mean([len(r["calls"]) for r in runs if r["passed"]]),
               "mean_compactions": _mean([r["compactions"] for r in runs]),
               "mean_minutes": _mean([r["latency_ms"] / 60000 for r in runs]),
               **move_measures(runs),
               "failures": [r["why"][:200] for r in runs if not r["passed"]][:3]}
        for name, runs in sorted(by_task.items())}}


def move_measures(runs):
    """Mean of each measure over runs whose runner recorded checkpoint places."""
    measured = [m for m in map(moves.measure, runs) if m]
    return {f"mean_{key}": _mean([m[key] for m in measured]) for key in moves.MEASURES}


def markdown(summary):
    s = summary["summary"]
    lines = [f"# Hard tasks: {summary.get('model')}", "",
             f"Via: {summary.get('via', 'runner')}. Runs: {s['runs']}; "
             f"infrastructure errors (excluded): {len(summary['errors'])}. "
             "Benchmark audit, not an automated check.", "",
             "| Task | Pass | Calls | Calls per pass | Compactions | Redone writes | Rewrites after checkpoint | "
             "Re-reads after checkpoint | Wasted repeats | Minutes | Failures |",
             "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for name, row in s["tasks"].items():
        lines.append(f"| {name} | {fmt(row['pass_rate'])} | {fmt(row['mean_calls'], False)} | "
                     f"{fmt(row.get('mean_calls_per_pass'), False)} | {fmt(row['mean_compactions'], False)} | "
                     + " | ".join(fmt(row.get(f"mean_{key}"), False) for key in moves.MEASURES) + " | "
                     f"{fmt(row['mean_minutes'], False)} | "
                     f"{'; '.join(row['failures']) or '-'} |")
    return "\n".join(lines) + "\n"


def run_via_app(binary, make, args):
    """One task as a real app chat, run once more after two minutes if the provider lost it."""
    task = make()
    # A long task's own limit wins over a shorter --timeout, as in the runner.
    timeout = max(args.timeout, task.get("timeout", 0))
    row, error = run_app_task(binary, task, args.model, timeout, args.keep)
    if error and not args.no_retry and (error.startswith("provider") or error == "timeout"):
        time.sleep(120)
        row, error = run_app_task(binary, make(), args.model, timeout, args.keep)
    return row, error, False


def run(args):
    if args.via == "app":
        if args.themis:
            binary = os.path.abspath(args.themis)
        else:
            built = subprocess.run(["cargo", "build", "-q", "-p", "themis-desktop", "--bin", "themis"], cwd=ROOT,
                                   check=False)
            if built.returncode != 0:
                return built.returncode
            binary = themis_binary(ROOT)

        def runner_call(make):
            return run_via_app(binary, make, args)
    else:
        runner = [os.path.abspath(args.runner)] if args.runner else cargo_build(
            ROOT, os.path.join(ROOT, os.environ.get("CARGO_TARGET_DIR", "target")))

        def runner_call(make):
            return run_task_retrying(runner, make, args)
    memories = ([f"b{budget}" for budget in args.budgets] if args.budgets
                else ["normal", "squeezed"] if args.memory == "both" else [args.memory])
    if args.budgets and args.via == "app" and max(args.budgets) > 256_000:
        print("--budgets above 256000 need --via runner: the app's settings stop at 256000", file=sys.stderr)
        return 2
    if args.nudges_off and args.via == "app":
        print("--nudges-off needs --via runner: the app has no eval switches", file=sys.stderr)
        return 2
    jobs = [(trial, job, memory) for trial in range(args.trials) for job in args.job for memory in memories]
    rows, errors = [], []
    os.makedirs(RESULTS, exist_ok=True)
    out = args.out or os.path.join(RESULTS, f"{datetime.date.today()}-{args.model}.json")

    def save():
        # Written after every run, so stopping a round early keeps what was graded.
        summary = {"suite": "hard", "model": args.model, "trials": args.trials, "via": args.via,
                   "created": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
                   "note": "Live-model benchmark audit; not an automated runtime check.",
                   "summary": summarize(rows), "errors": errors, "rows": rows}
        with open(out, "w", encoding="utf-8") as handle:
            json.dump(summary, handle, indent=2, ensure_ascii=False)
            handle.write("\n")
        return summary

    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        futures = {pool.submit(runner_call, lambda job=job, memory=memory:
                               dict(make_task(job, memory), nudges_off=args.nudges_off)):
                   (trial, job, memory) for trial, job, memory in jobs}
        for future in concurrent.futures.as_completed(futures):
            trial, job, memory = futures[future]
            try:
                row, error, _ = future.result()
            except Exception as failure:  # noqa: BLE001 - one run must never abort the suite
                row, error = None, f"harness: {type(failure).__name__}: {str(failure)[:160]}"
            if row and row.get("error_text", "").startswith("LLM request failed"):
                error = f"provider failed mid-run after {len(row['calls'])} calls: {row['error_text'][:120]}"
            name = f"{job}-{memory}"
            if error:
                errors.append({"trial": trial, "task": name, "error": error})
                print(f"trial {trial + 1} {name}: error {error}", flush=True)
            else:
                rows.append(dict(row, trial=trial))
                print(f"trial {trial + 1} {name}: {'pass' if row['passed'] else 'fail'} "
                      f"({row['why'] or 'all checks'}; {len(row['calls'])} calls, "
                      f"{row['compactions']} compactions)", flush=True)
            save()
    summary = save()
    print(markdown(summary))
    print(out)
    return 0


COMPARE = (("runs", "Runs"), ("pass_rate", "Pass"), ("mean_stages", "Stages"), ("mean_calls", "Calls"),
           ("mean_calls_per_pass", "Calls per pass"), ("mean_compactions", "Checkpoints"),
           ("mean_rereads_after_checkpoint", "Re-reads after checkpoint"), ("mean_wasted_repeats", "Wasted repeats"),
           ("mean_redone_writes", "Redone writes"), ("mean_rewrites_after_checkpoint", "Rewrites after checkpoint"))


def compare(paths):
    """Markdown table of several result files side by side, one row per file and task. A file
    can be labelled `name=path`; measures its runner did not record show n/a."""
    lines = ["| Arm | Task | " + " | ".join(title for _, title in COMPARE) + " |",
             "| --- | --- | " + " | ".join("---" for _ in COMPARE) + " |"]
    for item in paths:
        label, _, path = item.rpartition("=")
        label = label or os.path.splitext(os.path.basename(path))[0]
        with open(path, encoding="utf-8") as handle:
            rows = json.load(handle)["rows"]
        for name, row in summarize(rows)["tasks"].items():
            stages = [r["extra"]["stages_passed"] for r in rows
                      if r["task"] == name and "stages_passed" in (r.get("extra") or {})]
            row = dict(row, mean_stages=_mean(stages))
            cells = [fmt(row.get(key), key == "pass_rate") if key != "runs" else str(row["runs"])
                     for key, _ in COMPARE]
            lines.append(f"| {label} | {name} | " + " | ".join(cells) + " |")
    return "\n".join(lines) + "\n"


def main(argv=None):
    parser = argparse.ArgumentParser(prog="python3 -m evals.hard")
    sub = parser.add_subparsers(dest="command", required=True)
    cmd = sub.add_parser("run")
    cmd.add_argument("--job", action="append", choices=sorted(JOBS))
    cmd.add_argument("--trials", type=int, default=1)
    cmd.add_argument("--model", default="muse-spark-1.3-contributor")
    cmd.add_argument("--memory", choices=("normal", "squeezed", "both"), default="both")
    cmd.add_argument("--budgets", type=lambda text: [int(item) for item in text.split(",") if item],
                     help="compare context budgets, e.g. 64000,200000,1000000 (replaces --memory)")
    cmd.add_argument("--jobs", type=int, default=2)
    cmd.add_argument("--nudges-off", type=lambda text: [item for item in text.split(",") if item], default=[],
                     help="runner only: comma-separated runtime nudges to switch off (e.g. spec_check,loop_warning)")
    cmd.add_argument("--via", choices=("app", "runner"), default="app",
                     help="app: the real themis app server and CLI (default); runner: the tool-eval runner")
    cmd.add_argument("--max-turns", type=int, default=200)
    cmd.add_argument("--timeout", type=int, default=5400)
    cmd.add_argument("--base-url")
    cmd.add_argument("--runner", help="prebuilt tool_eval binary")
    cmd.add_argument("--themis", help="prebuilt themis CLI for --via app (default: build this checkout's)")
    cmd.add_argument("--sandbox", choices=("auto", "off"), default="auto")
    cmd.add_argument("--out")
    cmd.add_argument("--keep", help="save each run's .themis/ (full tool evidence) under this folder")
    cmd.add_argument("--no-retry", action="store_true", help="do not rerun runs lost to the provider")
    show = sub.add_parser("report")
    show.add_argument("result")
    side = sub.add_parser("compare", help="several result files side by side, with the checkpoint measures")
    side.add_argument("results", nargs="+", help="result files, each optionally labelled name=path")
    args = parser.parse_args(argv)
    if args.command == "compare":
        print(compare(args.results))
        return 0
    if args.command == "report":
        with open(args.result, encoding="utf-8") as handle:
            print(markdown(json.load(handle)))
        return 0
    args.job = args.job or ["sheet", "threejs"]  # the follow-up jobs run when named
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
