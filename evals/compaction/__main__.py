"""CLI: python3 -m evals.compaction run --suite core --trials 3 --model M --budget 24000"""

import argparse
import concurrent.futures
import datetime
import json
import os
import shlex
import subprocess
import sys

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
        case["directory"] = directory
        cases.append(case)
    return cases


def build_runner():
    command = ["cargo", "build", "--release", "-q", "-p", "themis-core", "--features", "eval",
               "--example", "compaction_eval"]
    subprocess.run(command, cwd=ROOT, check=True)
    target = os.environ.get("CARGO_TARGET_DIR", os.path.join(ROOT, "target"))
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

    trials, per_case, raw = [], {}, []
    for trial in range(args.trials):
        rows, runs = [], []
        for case in cases:
            report = results.get((trial, case["name"]))
            if not report:
                continue
            graded = trial_rows(case, report)
            rows += graded
            runs.append(report)
            per_case.setdefault(case["name"], []).append(
                sum(r["verdict"] == "correct" for r in graded if r["category"] != "decoy")
                / max(1, sum(r["category"] != "decoy" for r in graded)))
            raw.append({"trial": trial, "case": case["name"], "chunks": report["chunks"],
                        "rows": graded})
        if runs:
            trials.append(trial_summary(rows, runs))
    summary = {
        "suite": args.suite, "model": args.model, "budget": args.budget,
        "recent_messages": args.recent, "trials_requested": args.trials,
        "trials_with_data": len(trials), "cases": [c["name"] for c in cases],
        "failed_runs": failures, "created": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "git_commit": subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT,
                                     capture_output=True, text=True).stdout.strip(),
        "note": "Live-model benchmark audit; not an automated runtime check.",
        "aggregate": aggregate(trials) if trials else None,
        "per_trial": trials,
        "per_case_retention": {name: {"mean": round(sum(v) / len(v), 4), "trials": v}
                               for name, v in per_case.items()},
        "details": raw,
    }
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
    args = parser.parse_args(argv)
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
