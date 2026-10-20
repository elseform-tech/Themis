"""CLI: python3 -m evals.compaction run --suite core --trials 3 --model M --budget 24000"""

import argparse
import concurrent.futures
import datetime
import json
import os
import shlex
import subprocess
import sys

from . import behavior as behavior_grading
from .report import aggregate, trial_rows, trial_summary

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
CASES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "cases")
EXE = ".exe" if os.name == "nt" else ""


def load_cases(suite, only=None):
    cases = []
    for name in sorted(os.listdir(CASES)):
        directory = os.path.join(CASES, name)
        if not os.path.isfile(os.path.join(directory, "case.json")):
            continue
        with open(os.path.join(directory, "case.json"), encoding="utf-8") as handle:
            case = json.load(handle)
        if suite not in case.get("suites", []) or (only and name not in only):
            continue
        with open(os.path.join(directory, "probes.json"), encoding="utf-8") as handle:
            case["probes"] = json.load(handle)
        behavior_file = os.path.join(directory, "behavior.json")
        if os.path.isfile(behavior_file):
            with open(behavior_file, encoding="utf-8") as handle:
                case["behavior"] = json.load(handle)
        case["directory"] = directory
        cases.append(case)
    return cases


def target_dir():
    """Cargo's target directory; a relative CARGO_TARGET_DIR is relative to the repo root."""
    return os.path.join(ROOT, os.environ.get("CARGO_TARGET_DIR", "target"))


def recall_env():
    """`--cfg eval_recall` offers the read-only `recall` tool in agent turns, as the app does."""
    env = dict(os.environ)
    if os.path.isfile(os.path.join(ROOT, "crates/core/src/tools/recall.rs")):
        env["RUSTFLAGS"] = (env.get("RUSTFLAGS", "") + " --cfg eval_recall").strip()
    return env


def build_runner():
    command = ["cargo", "build", "--release", "-q", "-p", "themis-core", "--features", "eval",
               "--example", "compaction_eval"]
    subprocess.run(command, cwd=ROOT, check=True, env=recall_env())
    target = target_dir()
    return [os.path.join(target, "release", "examples", "compaction_eval" + EXE)]


def run_once(runner, case, args):
    command = runner + ["--case", case["directory"], "--budget", str(args.budget),
                        "--recent", str(args.recent)]
    if args.model:
        command += ["--model", args.model]
    if args.base_url:
        command += ["--base-url", args.base_url]
    try:
        done = subprocess.run(command, capture_output=True, text=True, timeout=args.timeout)
    except subprocess.TimeoutExpired:
        return None, f"timed out after {args.timeout}s"
    if done.returncode != 0:
        return None, (done.stderr or done.stdout).strip()[-400:]
    try:
        return json.loads(done.stdout), None
    except ValueError as error:
        return None, f"unparseable runner output: {error}"


def build_summary(cases, results, trials_requested):
    """Grades and aggregates `results[(trial, case_name)]` run reports.

    Runs whose compaction failed (`valid` false) were answered from uncompacted history;
    they are listed under `invalid_runs` and excluded from every retention statistic.
    """
    trials, per_case, raw, invalid = [], {}, [], []
    for trial in range(trials_requested):
        rows, runs = [], []
        for case in cases:
            report = results.get((trial, case["name"]))
            if not report:
                continue
            if report.get("valid") is False:
                invalid.append({"trial": trial, "case": case["name"],
                                "errors": [c.get("error") for c in report["chunks"]]})
                continue
            graded = trial_rows(case, report)
            rows += graded
            runs.append(report)
            scored = [r for r in graded if r["category"] != "decoy"]
            per_case.setdefault(case["name"], []).append(
                sum(r["verdict"] == "correct" for r in scored) / max(1, len(scored)))
            raw.append({"trial": trial, "case": case["name"], "chunks": report["chunks"],
                        "valid": True, "verbatim_records": report.get("verbatim_records"),
                        "rows": graded})
        if runs:
            trials.append(trial_summary(rows, runs))
    return {
        "trials_requested": trials_requested, "trials_with_data": len(trials),
        "cases": [c["name"] for c in cases], "invalid_runs": invalid,
        "aggregate": aggregate(trials) if trials else None,
        "per_trial": trials,
        "per_case_retention": {name: {"mean": round(sum(v) / len(v), 4), "trials": v}
                               for name, v in per_case.items()},
        "details": raw,
    }


def regrade(args):
    """Re-grades a stored result with the current grader and fixtures, offline."""
    with open(args.result, encoding="utf-8") as handle:
        old = json.load(handle)
    cases = {c["name"]: c for c in load_cases(old["suite"])}
    results = {}
    for item in old["details"]:
        results[(item["trial"], item["case"])] = {
            "chunks": item["chunks"], "valid": item.get("valid", True),
            "verbatim_records": item.get("verbatim_records"),
            "answers": [{"id": r["id"], "answer": r["answer"], "error": r.get("error")}
                        for r in item["rows"]]}
    summary = build_summary([cases[n] for n in old["cases"]], results, old["trials_requested"])
    for key in ("suite", "model", "budget", "recent_messages", "failed_runs", "created",
                "git_commit"):
        summary[key] = old.get(key)
    summary["note"] = ("Offline regrade of " + os.path.basename(args.result) + " with the "
                       "current grader and fixtures; answers are the original model output. "
                       + ("" if summary["aggregate"] and summary["aggregate"]["tail_split_available"]
                          else "Verbatim-record data was not stored by that run, so no "
                               "summary-only/tail split is available."))
    out = args.out or args.result.replace(".json", "-regraded.json")
    with open(out, "w", encoding="utf-8") as handle:
        json.dump(summary, handle, indent=2, ensure_ascii=False)
        handle.write("\n")
    print(out)
    return 0


def build_behavior_runner():
    command = ["cargo", "build", "--release", "-q", "-p", "themis-core", "--features", "eval",
               "--example", "behavior_eval"]
    subprocess.run(command, cwd=ROOT, check=True, env=recall_env())
    return [os.path.join(target_dir(), "release", "examples", "behavior_eval" + EXE)]


def behavior(args):
    """Behavior evals: the next user message after a compaction (greeting, unrelated, return, continue)."""
    cases = load_cases("behavior", args.case)
    if not cases:
        sys.exit("no cases in suite 'behavior'")
    runner = ([args.runner] if args.runner and os.path.exists(args.runner)
              else shlex.split(args.runner) if args.runner else build_behavior_runner())
    jobs = [(trial, case) for trial in range(args.trials) for case in cases]
    rows, failures, raw = [], [], []

    def work(job):
        trial, case = job
        command = runner + ["--case", case["directory"], "--budget", str(args.budget),
                            "--recent", str(args.recent), "--anchors", args.anchors]
        if args.model:
            command += ["--model", args.model]
        if args.effort:
            command += ["--effort", args.effort]
        if args.base_url:
            command += ["--base-url", args.base_url]
        try:
            done = subprocess.run(command, capture_output=True, text=True, timeout=args.timeout)
            if done.returncode != 0:
                return job, None, (done.stderr or done.stdout).strip()[-400:]
            return job, json.loads(done.stdout), None
        except (subprocess.TimeoutExpired, ValueError) as error:
            return job, None, str(error)

    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        for (trial, case), report, error in pool.map(work, jobs):
            if not report or report.get("valid") is False:
                failures.append({"trial": trial, "case": case["name"], "error": error or "invalid"})
                continue
            graded = behavior_grading.grade_run(case, report)
            rows += graded
            raw.append({"trial": trial, "case": case["name"], "turns": report["turns"], "graded": graded})
    summary = {"suite": "behavior", "anchors": args.anchors, "model": args.model, "effort": args.effort,
               "budget": args.budget,
               "trials_requested": args.trials, "failed_runs": failures,
               "summary": behavior_grading.summarize(rows),
               "created": datetime.datetime.now(datetime.timezone.utc).isoformat(),
               "git_commit": subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT,
                                            capture_output=True, text=True).stdout.strip(),
               "note": "Live-model behavior audit; not an automated runtime check.", "details": raw}
    out = args.out or os.path.join(
        os.path.dirname(os.path.abspath(__file__)), "results",
        f"{datetime.date.today()}-behavior-{args.anchors}-{(args.model or 'default').replace('/', '_')}-{args.budget}.json")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "w", encoding="utf-8") as handle:
        json.dump(summary, handle, indent=2, ensure_ascii=False)
        handle.write("\n")
    print(out)
    return 1 if failures else 0


def run(args):
    cases = load_cases(args.suite, args.case)
    if not cases:
        sys.exit(f"no cases in suite {args.suite!r}")
    if not args.runner:
        runner = build_runner()
    elif os.path.exists(args.runner):  # a bare path; shlex would eat Windows backslashes
        runner = [args.runner]
    else:
        runner = shlex.split(args.runner)
    jobs = [(trial, case) for trial in range(args.trials) for case in cases]
    results, failures = {}, []

    def work(job):
        trial, case = job
        report, error = run_once(runner, case, args)
        print(f"trial {trial + 1} {case['name']}: {'ok' if report else 'FAILED ' + str(error)}",
              file=sys.stderr, flush=True)
        return job, report, error

    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        for (trial, case), report, error in pool.map(work, jobs):
            if report:
                results[(trial, case["name"])] = report
            else:
                failures.append({"trial": trial, "case": case["name"], "error": error})

    summary = build_summary(cases, results, args.trials)
    summary.update({
        "suite": args.suite, "model": args.model, "budget": args.budget,
        "recent_messages": args.recent, "failed_runs": failures,
        "created": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "git_commit": subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT,
                                     capture_output=True, text=True).stdout.strip(),
        "note": "Live-model benchmark audit; not an automated runtime check.",
    })
    out = args.out or os.path.join(
        os.path.dirname(os.path.abspath(__file__)), "results",
        f"{datetime.date.today()}-{(args.model or 'default').replace('/', '_')}-{args.budget}.json")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "w", encoding="utf-8") as handle:
        json.dump(summary, handle, indent=2, ensure_ascii=False)
        handle.write("\n")
    print(out)
    return 1 if failures else 0


def main(argv=None):
    parser = argparse.ArgumentParser(prog="python3 -m evals.compaction")
    sub = parser.add_subparsers(dest="command", required=True)
    cmd = sub.add_parser("run", help="run every case x trial against a model and grade it")
    cmd.add_argument("--suite", default="core")
    cmd.add_argument("--case", action="append", help="restrict to this case name (repeatable)")
    cmd.add_argument("--trials", type=int, default=3)
    cmd.add_argument("--model", default=None)
    cmd.add_argument("--budget", type=int, default=24000)
    cmd.add_argument("--recent", type=int, default=20)
    cmd.add_argument("--base-url", default=None, help="point the provider at a mock server")
    cmd.add_argument("--jobs", type=int, default=3)
    cmd.add_argument("--timeout", type=int, default=1200)
    cmd.add_argument("--runner", default=None, help="runner command (default: build the example)")
    cmd.add_argument("--out", default=None)
    beh = sub.add_parser("behavior", help="run the behavior evals (greeting, unrelated, return, continue)")
    beh.add_argument("--case", action="append")
    beh.add_argument("--trials", type=int, default=3)
    beh.add_argument("--model", default=None)
    beh.add_argument("--budget", type=int, default=24000)
    beh.add_argument("--recent", type=int, default=20)
    beh.add_argument("--anchors", choices=["old", "new"], default="new",
                     help="old: previous desktop anchors for every message; new: skipped for social turns")
    beh.add_argument("--base-url", default=None)
    beh.add_argument("--jobs", type=int, default=3)
    beh.add_argument("--timeout", type=int, default=1200)
    beh.add_argument("--runner", default=None)
    beh.add_argument("--effort", default=None, help="reasoning effort sent to the provider (e.g. max)")
    beh.add_argument("--out", default=None)
    again = sub.add_parser("regrade", help="re-grade a stored result offline")
    again.add_argument("result")
    again.add_argument("--out", default=None)
    args = parser.parse_args(argv)
    if args.command == "behavior":
        return behavior(args)
    return regrade(args) if args.command == "regrade" else run(args)


if __name__ == "__main__":
    sys.exit(main())
