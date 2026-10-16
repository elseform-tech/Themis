"""Real-project eval: a five-message coding job on a seeded Python project, graded per stage.

    python3 -m evals.project run --trials 3 --model longcat-2.5-preview-free [--memory normal]

Live-model benchmark audit; not an automated runtime check. Each trial gets a new seed (package
name, catalog, orders and hidden cases), a fresh scratch project, a scratch HOME and, on macOS,
`sandbox-exec` around the runner. The grader runs agent code only in sandboxed scratch copies.
"""

import argparse
import collections
import concurrent.futures
import datetime
import json
import os
import shutil
import sys
import tempfile

from evals.tools.__main__ import ROOT, _mean, cargo_build, fmt, run_task_retrying
from evals.tools.tasks import commit_all

from .generate import generate
from .grade import failure_cause, grade
from .tasks import prompts

RESULTS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "results")
# The app's defaults (200,000-token context, 200 turns); "squeezed" forces many checkpoints.
MEMORY = {"normal": {"budget": 200_000, "recent_messages": 20},
          "squeezed": {"budget": 32_000, "recent_messages": 8}}
STAGES = ["1_parser_pricing", "2_progress", "3_bug_fix", "4_cli_docs", "5_recall"]


def make_task(seed, memory, arm):
    """One trial's task: the project for `seed`, its five messages and its grader."""
    graded = {}
    holder = {}

    def setup(project):
        holder["facts"] = generate(project, seed)
        commit_all(project, "Order quotes 1.4")
        messages = prompts(holder["facts"])
        task["prompt"], task["followups"] = messages[0], messages[1:]

    def check(project, report):
        answers = [turn.get("answer", "") for turn in report.get("turns", [])]
        snapshots = {turn: f"{project}.turn-{turn}" for turn in range(1, 6)}
        # The untouched project is regenerated from the seed, out of the agent's reach.
        original = tempfile.mkdtemp(prefix="project-original-")
        try:
            generate(original, seed)
            passed, stages = grade(original, snapshots, answers, holder["facts"])
        finally:
            shutil.rmtree(original, ignore_errors=True)
        cause = failure_cause(stages, [turn.get("error") for turn in report.get("turns", [])])
        graded.update(stages=stages, cause=cause)
        return passed, cause or ""

    def measure(project, report):
        stages = graded.get("stages", {})
        final = stages.get("4_cli_docs", {}).get("hidden", {})
        run = sum(r.get("run", 0) for r in final.values())
        failed = sum(len(r.get("failed", [])) for r in final.values())
        return {"seed": seed, "package": holder["facts"]["package"], "cause": graded.get("cause"),
                "stage_ok": {name: bool(stages.get(name, {}).get("ok")) for name in STAGES},
                "final_hidden_pass": (run - failed) / run if run else 0.0,
                "rules_broken": sorted({p for s in stages.values() for p in s.get("rules", [])})[:10],
                "tests_added": stages.get("1_parser_pricing", {}).get("tests_added"),
                "recalled": stages.get("5_recall", {}).get("recalled"),
                "loop_warnings": sum(t.get("loop_warnings") or 0 for t in report.get("turns", [])),
                "finish_checks": sum(t.get("finish_checks") or 0 for t in report.get("turns", []))}

    task = {"name": f"project-{memory}-{arm}", "tool": "project", "job": "project", "arm": arm, "memory": memory,
            "setup": setup, "check": check, "measure": measure, "prompt": "", "followups": [], "snapshots": True,
            "evidence": ".themis/context/eval-thread", "offer_notes": arm == "notes", "timeout": 5400,
            # YOLO-style approvals as a developer would use them; the OS sandbox still applies.
            "allow_all": True,
            **MEMORY[memory]}
    return task


def summarize(rows):
    by_task = collections.defaultdict(list)
    for row in rows:
        by_task[row["task"]].append(row)
    tasks = {}
    for name, runs in sorted(by_task.items()):
        extra = [r["extra"] for r in runs]
        tasks[name] = {
            "runs": len(runs), "pass_rate": _mean([r["passed"] for r in runs]),
            "stage_pass": {stage: _mean([e["stage_ok"].get(stage) for e in extra]) for stage in STAGES},
            "final_hidden_pass": _mean([e["final_hidden_pass"] for e in extra]),
            "causes": dict(collections.Counter(e["cause"] for e in extra if e["cause"])),
            "mean_loop_warnings": _mean([e["loop_warnings"] for e in extra]),
            "mean_finish_checks": _mean([e.get("finish_checks") for e in extra]),
            "mean_compactions": _mean([r["compactions"] for r in runs]),
            "notes_used_rate": _mean([any(n for n in r.get("notes_log", [])) for r in runs]),
            "mean_calls": _mean([len(r["calls"]) for r in runs]),
            "mean_input_tokens": _mean([r.get("input_tokens") for r in runs]),
            "mean_minutes": _mean([r["latency_ms"] / 60000 for r in runs]),
        }
    return {"runs": len(rows), "pass_rate": _mean([r["passed"] for r in rows]), "tasks": tasks}


def markdown(summary):
    s = summary["summary"]
    lines = [f"# Real project: {summary.get('model')}", "",
             f"Runs: {s['runs']}; infrastructure errors (excluded): {len(summary['errors'])}; "
             f"runs retried after a provider error: {summary.get('retried', 0)}. "
             "Benchmark audit, not an automated check.", "",
             "| Setup | Pass | 1 parser+pricing | 2 progress | 3 bug fix | 4 CLI+docs | 5 recall | Hidden tests at end | "
             "Loop warnings | Open-task checks | Compactions | Notes used | Calls | Minutes | Failure causes |",
             "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for name, row in s["tasks"].items():
        stages = " | ".join(fmt(row["stage_pass"][stage]) for stage in STAGES)
        causes = ", ".join(f"{k} {v}" for k, v in row["causes"].items()) or "-"
        lines.append(f"| {name} | {fmt(row['pass_rate'])} | {stages} | {fmt(row['final_hidden_pass'])} | "
                     f"{fmt(row['mean_loop_warnings'], False)} | {fmt(row.get('mean_finish_checks'), False)} | "
                     f"{fmt(row['mean_compactions'], False)} | "
                     f"{fmt(row['notes_used_rate'])} | {fmt(row['mean_calls'], False)} | "
                     f"{fmt(row['mean_minutes'], False)} | {causes} |")
    return "\n".join(lines) + "\n"


def run(args):
    runner = [os.path.abspath(args.runner)] if args.runner else cargo_build(
        ROOT, os.path.join(ROOT, os.environ.get("CARGO_TARGET_DIR", "target")))
    memories = ["normal", "squeezed"] if args.memory == "both" else [args.memory]
    jobs = [(trial, args.seed + trial, memory, arm)
            for trial in range(args.trials) for memory in memories for arm in args.arm]
    rows, errors, retries = [], [], 0
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        futures = {}
        for trial, seed, memory, arm in jobs:
            make = lambda seed=seed, memory=memory, arm=arm: make_task(seed, memory, arm)  # noqa: E731
            futures[pool.submit(run_task_retrying, runner, make, args)] = (trial, make_task(seed, memory, arm))
        for future in concurrent.futures.as_completed(futures):
            trial, task = futures[future]
            row, error, retried = future.result()
            retries += retried
            if row and row.get("error_text", "").startswith("LLM request failed"):
                error = f"provider failed mid-run after {len(row['calls'])} calls: {row['error_text'][:120]}"
            if error:
                errors.append({"trial": trial, "task": task["name"], "error": error, **({"row": row} if row else {})})
                print(f"trial {trial + 1} {task['name']}: error {error}", flush=True)
            else:
                rows.append(dict(row, trial=trial, arm=task["arm"], memory=task["memory"]))
                print(f"trial {trial + 1} {task['name']}: {'pass' if row['passed'] else 'fail'} "
                      f"({row['why'] or 'all stages'}; {row['compactions']} compactions)", flush=True)
    summary = {"suite": "project", "model": args.model, "trials": args.trials,
               "created": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
               "note": "Live-model benchmark audit; not an automated runtime check.",
               "summary": summarize(rows), "errors": errors, "retried": retries, "rows": rows}
    os.makedirs(RESULTS, exist_ok=True)
    out = args.out or os.path.join(RESULTS, f"{datetime.date.today()}-{args.model or 'mock'}.json")
    with open(out, "w", encoding="utf-8") as handle:
        json.dump(summary, handle, indent=2, ensure_ascii=False)
        handle.write("\n")
    print(markdown(summary))
    print(out)
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(prog="python3 -m evals.project")
    sub = parser.add_subparsers(dest="command", required=True)
    cmd = sub.add_parser("run")
    cmd.add_argument("--trials", type=int, default=3)
    cmd.add_argument("--seed", type=int, default=1, help="first trial's seed; later trials add one")
    cmd.add_argument("--model", default="longcat-2.5-preview-free")
    cmd.add_argument("--memory", choices=("normal", "squeezed", "both"), default="normal")
    cmd.add_argument("--arm", action="append", choices=("notes", "no-notes"))
    cmd.add_argument("--jobs", type=int, default=2)
    cmd.add_argument("--max-turns", type=int, default=200)
    cmd.add_argument("--timeout", type=int, default=5400)
    cmd.add_argument("--base-url")
    cmd.add_argument("--runner", help="prebuilt tool_eval binary")
    cmd.add_argument("--sandbox", choices=("auto", "off"), default="auto")
    cmd.add_argument("--out")
    cmd.add_argument("--no-retry", action="store_true", help="do not rerun runs lost to the provider")
    show = sub.add_parser("report")
    show.add_argument("result")
    args = parser.parse_args(argv)
    if args.command == "report":
        with open(args.result, encoding="utf-8") as handle:
            print(markdown(json.load(handle)))
        return 0
    args.arm = args.arm or ["notes", "no-notes"]
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
