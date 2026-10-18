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

from . import sheet, threejs
from .app import run_app_task, themis_binary

RESULTS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "results")
# The app's defaults, and a small budget that forces many checkpoints mid-task.
MEMORY = {"normal": {"budget": 200_000, "recent_messages": 20},
          "squeezed": {"budget": 32_000, "recent_messages": 8}}
JOBS = {"threejs": threejs, "sheet": sheet}


def make_task(job, memory, notes=True):
    module = JOBS[job]
    return {"name": f"{job}-{memory}" + ("" if notes else "-no-notes"), "tool": job, "job": job, "memory": memory,
            "notes": notes,
            "setup": module.setup, "check": module.check, "prompt": module.PROMPT,
            "evidence": ".themis/context/eval-thread", "offer_notes": notes, "timeout": 5400,
            # YOLO-style approvals as a developer would use them; the OS sandbox still applies.
            "allow_all": True, **MEMORY[memory]}


def summarize(rows):
    by_task = collections.defaultdict(list)
    for row in rows:
        by_task[row["task"]].append(row)
    return {"runs": len(rows), "pass_rate": _mean([r["passed"] for r in rows]), "tasks": {
        name: {"runs": len(runs), "pass_rate": _mean([r["passed"] for r in runs]),
               "mean_calls": _mean([len(r["calls"]) for r in runs]),
               "mean_compactions": _mean([r["compactions"] for r in runs]),
               # Notes carried by every checkpoint: the goal and task lines survive compaction.
               "checkpoints_with_notes": _mean([notes_kept(r) for r in runs if r.get("checkpoints")]),
               "mean_minutes": _mean([r["latency_ms"] / 60000 for r in runs]),
               "failures": [r["why"][:200] for r in runs if not r["passed"]][:3]}
        for name, runs in sorted(by_task.items())}}


def notes_kept(row):
    """Share of the run's checkpoints that carried notes lines (None without checkpoints)."""
    points = row.get("checkpoints") or []
    if not points:
        return None
    return sum(1 for point in points if point.get("notes")) / len(points)


def markdown(summary):
    s = summary["summary"]
    lines = [f"# Hard tasks: {summary.get('model')}", "",
             f"Via: {summary.get('via', 'runner')}. Runs: {s['runs']}; "
             f"infrastructure errors (excluded): {len(summary['errors'])}. "
             "Benchmark audit, not an automated check.", "",
             "| Task | Pass | Calls | Compactions | Checkpoints with notes | Minutes | Failures |",
             "| --- | --- | --- | --- | --- | --- | --- |"]
    for name, row in s["tasks"].items():
        lines.append(f"| {name} | {fmt(row['pass_rate'])} | {fmt(row['mean_calls'], False)} | "
                     f"{fmt(row['mean_compactions'], False)} | {fmt(row.get('checkpoints_with_notes'))} | "
                     f"{fmt(row['mean_minutes'], False)} | "
                     f"{'; '.join(row['failures']) or '-'} |")
    return "\n".join(lines) + "\n"


def run_via_app(binary, make, args):
    """One task as a real app chat, run once more after two minutes if the provider lost it."""
    row, error = run_app_task(binary, make(), args.model, args.timeout, args.keep)
    if error and not args.no_retry and (error.startswith("provider") or error == "timeout"):
        time.sleep(120)
        row, error = run_app_task(binary, make(), args.model, args.timeout, args.keep)
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
    memories = ["normal", "squeezed"] if args.memory == "both" else [args.memory]
    if args.no_notes and args.via == "app":
        print("--no-notes needs --via runner: the app always offers notes", file=sys.stderr)
        return 2
    notes_arms = [True, False] if args.no_notes else [True]
    jobs = [(trial, job, memory, notes) for trial in range(args.trials) for job in args.job for memory in memories
            for notes in notes_arms]
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
        futures = {pool.submit(runner_call, lambda job=job, memory=memory, notes=notes: make_task(job, memory, notes)):
                   (trial, job, memory, notes) for trial, job, memory, notes in jobs}
        for future in concurrent.futures.as_completed(futures):
            trial, job, memory, notes = futures[future]
            row, error, _ = future.result()
            if row and row.get("error_text", "").startswith("LLM request failed"):
                error = f"provider failed mid-run after {len(row['calls'])} calls: {row['error_text'][:120]}"
            name = f"{job}-{memory}" + ("" if notes else "-no-notes")
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


def main(argv=None):
    parser = argparse.ArgumentParser(prog="python3 -m evals.hard")
    sub = parser.add_subparsers(dest="command", required=True)
    cmd = sub.add_parser("run")
    cmd.add_argument("--job", action="append", choices=sorted(JOBS))
    cmd.add_argument("--trials", type=int, default=1)
    cmd.add_argument("--model", default="muse-spark-1.3-contributor")
    cmd.add_argument("--memory", choices=("normal", "squeezed", "both"), default="both")
    cmd.add_argument("--jobs", type=int, default=2)
    cmd.add_argument("--no-notes", action="store_true",
                     help="also run each task with the notes tool withheld (needs --via runner)")
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
    args = parser.parse_args(argv)
    if args.command == "report":
        with open(args.result, encoding="utf-8") as handle:
            print(markdown(json.load(handle)))
        return 0
    args.job = args.job or sorted(JOBS)
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
