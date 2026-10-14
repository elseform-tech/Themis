"""CLI: python3 -m evals.longform run --trials 3 --model M --budget 24000 (live benchmark audit)."""

import argparse
import concurrent.futures
import datetime
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile

from .grade import grade
from .report import aggregate, markdown, trial_summary
from .validate import context_location, validate

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
HERE = os.path.dirname(os.path.abspath(__file__))
CASES = os.path.join(HERE, "cases")
EXE = ".exe" if os.name == "nt" else ""
OVERLAY = ["crates/core/src/runtime/eval.rs", "crates/core/examples/longform_eval.rs"]
EXAMPLE_STANZA = '\n[[example]]\nname = "longform_eval"\nrequired-features = ["eval"]\n'


def load_cases(suite="longform", only=None):
    cases = []
    for name in sorted(os.listdir(CASES)):
        directory = os.path.join(CASES, name)
        if not os.path.isfile(os.path.join(directory, "case.json")):
            continue
        with open(os.path.join(directory, "case.json"), encoding="utf-8") as handle:
            case = json.load(handle)
        if suite not in case.get("suites", []) or (only and case["name"] not in only and name not in only):
            continue
        with open(os.path.join(directory, "probes.json"), encoding="utf-8") as handle:
            case["probes"] = json.load(handle)
        case["directory"] = directory
        cases.append(case)
    return cases


def cargo_build(cwd, target):
    env = dict(os.environ, CARGO_TARGET_DIR=target)
    subprocess.run(["cargo", "build", "--release", "-q", "-p", "themis-core", "--features", "eval",
                    "--example", "longform_eval"], cwd=cwd, check=True, env=env)
    return [os.path.join(target, "release", "examples", "longform_eval" + EXE)]


def build_runner():
    return cargo_build(ROOT, os.path.join(ROOT, os.environ.get("CARGO_TARGET_DIR", "target")))


def build_ref_runner(ref):
    """Builds the runner against another git ref (e.g. a branch with a `recall` tool).

    The ref is checked out in a temporary worktree and only the eval entry points (`eval.rs`,
    the example and its Cargo stanza) are overlaid from this checkout, so the tools, runtime and
    compaction under test are the ref's own. Returns (runner command, cleanup callback).
    """
    tree = tempfile.mkdtemp(prefix="longform-ref-")
    shutil.rmtree(tree)
    subprocess.run(["git", "worktree", "add", "--detach", tree, ref], cwd=ROOT, check=True)

    def cleanup():
        subprocess.run(["git", "worktree", "remove", "--force", tree], cwd=ROOT, check=False)

    try:
        for relative in OVERLAY:
            os.makedirs(os.path.dirname(os.path.join(tree, relative)), exist_ok=True)
            shutil.copy(os.path.join(ROOT, relative), os.path.join(tree, relative))
        manifest = os.path.join(tree, "crates/core/Cargo.toml")
        with open(manifest, encoding="utf-8") as handle:
            text = handle.read()
        if 'name = "longform_eval"' not in text:
            with open(manifest, "a", encoding="utf-8") as handle:
                handle.write(EXAMPLE_STANZA)
        target = os.path.join(ROOT, "target", "longform-ref")
        runner = cargo_build(tree, target)
        # The binary is self-contained; keep a copy so the worktree can go.
        kept = os.path.join(tempfile.mkdtemp(prefix="longform-bin-"), os.path.basename(runner[0]))
        shutil.copy(runner[0], kept)
        return [kept], cleanup
    except BaseException:
        cleanup()
        raise


def process_run(case, report, workdir):
    """Grades one runner report and validates citations while the evidence still exists."""
    evidence = os.path.join(workdir, "project", ".themis", "context", "eval-thread")
    messages = []
    context = os.path.join(workdir, "context.json")
    if os.path.exists(context):
        with open(context, encoding="utf-8") as handle:
            messages = json.load(handle)
    answers = {a["id"]: a for a in report.get("answers", [])}
    available = sorted({tool for a in answers.values() for tool in a.get("tools_available", [])})
    rows = []
    for probe in case["probes"]:
        entry = answers.get(probe["id"], {})
        answer = entry.get("answer") if entry.get("finished") or entry.get("answer") else None
        decoy = probe["category"] == "decoy"
        cite = {"present": False, "valid": False, "cited": []}
        if not decoy and answer:
            cite = validate(evidence, probe, answer)
        tools = entry.get("tools", [])
        rows.append({
            "case": case["name"], "id": probe["id"], "category": probe["category"], "domain": case["kind"],
            "in_context": None if decoy else context_location(messages, probe),
            "verdict": grade(probe, answer), "answer": answer or "", "error": entry.get("error"),
            "cite_present": cite["present"], "cite_valid": cite["valid"], "cited": cite["cited"],
            "tools": tools, "tool_calls": sum(t != "update_plan" for t in tools),
            "denied": entry.get("denied", 0), "used_recall": "recall" in tools,
            "requests": entry.get("requests", 0), "input_tokens": entry.get("input_tokens"),
            "output_tokens": entry.get("output_tokens"), "latency_ms": entry.get("latency_ms", 0),
        })
    report["recall_available"] = "recall" in available
    report["tools_available"] = available
    return rows


def run_once(runner, case, args):
    workdir = tempfile.mkdtemp(prefix="longform-run-")  # system temp: outside the repo and ~/Documents
    try:
        command = runner + ["--case", case["directory"], "--workdir", workdir, "--budget", str(args.budget),
                            "--recent", str(args.recent), "--max-calls", str(args.max_calls)]
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
            report = json.loads(done.stdout)
        except ValueError as error:
            return None, f"unparseable runner output: {error}"
        report["rows"] = process_run(case, report, workdir)
        return report, None
    finally:
        shutil.rmtree(workdir, ignore_errors=True)


def build_summary(cases, results, trials_requested):
    """Aggregates `results[(trial, case_name)]` reports; trials whose compaction failed are invalid."""
    trials, raw, invalid = [], [], []
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
            rows += report["rows"]
            runs.append(report)
            raw.append({"trial": trial, "case": case["name"], "chunks": report["chunks"],
                        "tools_available": report.get("tools_available"), "rows": report["rows"]})
        if runs:
            trials.append(trial_summary(rows, runs))
    return {"trials_requested": trials_requested, "trials_with_data": len(trials),
            "cases": [c["name"] for c in cases], "invalid_runs": invalid,
            "aggregate": aggregate(trials) if trials else None, "per_trial": trials, "details": raw}


def run(args):
    cases = load_cases("longform", args.case)
    if not cases:
        sys.exit("no cases selected")
    cleanup = None
    if args.runner:
        runner = [args.runner] if os.path.exists(args.runner) else shlex.split(args.runner)
    elif args.build_ref:
        runner, cleanup = build_ref_runner(args.build_ref)
    else:
        runner = build_runner()
    jobs = [(trial, case) for trial in range(args.trials) for case in cases]
    results, failures = {}, []

    def work(job):
        trial, case = job
        report, error = run_once(runner, case, args)
        print(f"trial {trial + 1} {case['name']}: {'ok' if report else 'FAILED ' + str(error)}",
              file=sys.stderr, flush=True)
        return job, report, error

    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
            for (trial, case), report, error in pool.map(work, jobs):
                if report:
                    results[(trial, case["name"])] = report
                else:
                    failures.append({"trial": trial, "case": case["name"], "error": error})
    finally:
        if cleanup:
            cleanup()
    summary = build_summary(cases, results, args.trials)
    summary.update({
        "suite": "longform", "model": args.model, "budget": args.budget, "recent_messages": args.recent,
        "max_tool_calls": args.max_calls, "failed_runs": failures, "build_ref": args.build_ref,
        "created": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "git_commit": subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, capture_output=True,
                                     text=True).stdout.strip(),
        "note": "Live-model benchmark audit; not an automated runtime check.",
    })
    out = args.out or os.path.join(HERE, "results", f"{datetime.date.today()}-{(args.model or 'default').replace('/', '_')}-{args.budget}.json")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "w", encoding="utf-8") as handle:
        json.dump(summary, handle, indent=2, ensure_ascii=False)
        handle.write("\n")
    print(markdown(summary))
    print(out)
    return 1 if failures else 0


def show(args):
    with open(args.result, encoding="utf-8") as handle:
        print(markdown(json.load(handle)))
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(prog="python3 -m evals.longform")
    sub = parser.add_subparsers(dest="command", required=True)
    cmd = sub.add_parser("run", help="run every case x trial in agent mode and grade it")
    cmd.add_argument("--case", action="append", help="restrict to this case name (repeatable)")
    cmd.add_argument("--trials", type=int, default=3)
    cmd.add_argument("--model", default=None)
    cmd.add_argument("--budget", type=int, default=24000)
    cmd.add_argument("--recent", type=int, default=20)
    cmd.add_argument("--max-calls", type=int, default=8, help="tool calls allowed per probe")
    cmd.add_argument("--base-url", default=None, help="point the provider at a mock server")
    cmd.add_argument("--jobs", type=int, default=3)
    cmd.add_argument("--timeout", type=int, default=3600)
    group = cmd.add_mutually_exclusive_group()
    group.add_argument("--runner", default=None, help="prebuilt longform_eval binary (skips the build)")
    group.add_argument("--build-ref", default=None, help="git ref to build the tools/runtime from (eval entry points overlaid)")
    cmd.add_argument("--out", default=None)
    again = sub.add_parser("report", help="print the markdown summary of a stored result")
    again.add_argument("result")
    args = parser.parse_args(argv)
    return show(args) if args.command == "report" else run(args)


if __name__ == "__main__":
    sys.exit(main())
