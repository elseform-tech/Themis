"""Tool-use eval: runs each task with the real chat toolset and measures how tool calls go.

    python3 -m evals.tools run --trials 3 --model longcat-2.5-preview-free [--build-ref main]

Live-model benchmark audit; not an automated runtime check. Each run gets a fresh scratch
project in the system temp folder, a scratch HOME and, on macOS, `sandbox-exec` around the
runner (as in the long-form suite).
"""

import argparse
import collections
import concurrent.futures
import datetime
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile

from evals.longform.__main__ import isolated_env, sandbox_wrapper
from evals.longform.report import classify_error

from .grade import call_rows
from .tasks import TASKS, prepare

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
RESULTS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "results")
EXE = ".exe" if os.name == "nt" else ""
OVERLAY = ["crates/core/src/runtime/tool_eval.rs", "crates/core/examples/tool_eval.rs"]
STANZA = '\n[[example]]\nname = "tool_eval"\nrequired-features = ["eval"]\n'
MODULE = '#[cfg(feature = "eval")]\npub mod tool_eval;\n'


def cargo_build(cwd, target):
    env = dict(os.environ, CARGO_TARGET_DIR=target)
    if os.path.isfile(os.path.join(cwd, "crates/core/src/tools/recall.rs")):
        env["RUSTFLAGS"] = (env.get("RUSTFLAGS", "") + " --cfg eval_recall").strip()
    subprocess.run(["cargo", "build", "--release", "-q", "-p", "themis-core", "--features", "eval",
                    "--example", "tool_eval"], cwd=cwd, check=True, env=env)
    return [os.path.join(target, "release", "examples", "tool_eval" + EXE)]


def build_ref_runner(ref):
    """Builds the runner against `ref` (its own tools and runtime) with only the eval entry
    points overlaid from this checkout. Returns (runner command, cleanup)."""
    tree = tempfile.mkdtemp(prefix="tools-ref-")
    shutil.rmtree(tree)
    subprocess.run(["git", "worktree", "add", "--detach", tree, ref], cwd=ROOT, check=True)

    def cleanup():
        subprocess.run(["git", "worktree", "remove", "--force", tree], cwd=ROOT, check=False)

    try:
        for relative in OVERLAY:
            shutil.copy(os.path.join(ROOT, relative), os.path.join(tree, relative))
        manifest = os.path.join(tree, "crates/core/Cargo.toml")
        with open(manifest, encoding="utf-8") as handle:
            if 'name = "tool_eval"' not in handle.read():
                with open(manifest, "a", encoding="utf-8") as out:
                    out.write(STANZA)
        runtime = os.path.join(tree, "crates/core/src/runtime.rs")
        with open(runtime, encoding="utf-8") as handle:
            text = handle.read()
        if "pub mod tool_eval;" not in text:
            with open(runtime, "w", encoding="utf-8") as handle:
                handle.write(text.replace('#[cfg(feature = "eval")]\npub mod eval;\n',
                                          '#[cfg(feature = "eval")]\npub mod eval;\n' + MODULE, 1))
        runner = cargo_build(tree, os.path.join(ROOT, "target", "tools-ref"))
        kept = os.path.join(tempfile.mkdtemp(prefix="tools-bin-"), os.path.basename(runner[0]))
        shutil.copy(runner[0], kept)
        return [kept], cleanup
    except BaseException:
        cleanup()
        raise


def run_task(runner, task, args):
    """One task in a fresh scratch project: (graded row, error class or None)."""
    workdir = tempfile.mkdtemp(prefix="tools-run-")
    try:
        project = os.path.join(workdir, "project")
        prepare(task, project)
        task_file = os.path.join(workdir, "task.json")
        with open(task_file, "w", encoding="utf-8") as handle:
            json.dump({k: task[k] for k in ("prompt", "checkpoint", "evidence", "budget", "recent_messages",
                                            "max_turns", "offer_plan") if k in task}, handle)
        command = runner + ["--project", project, "--task", task_file, "--max-turns", str(args.max_turns)]
        if args.model:
            command += ["--model", args.model]
        if args.base_url:
            command += ["--base-url", args.base_url]
        command = sandbox_wrapper(runner, workdir, args.sandbox) + command
        try:
            done = subprocess.run(command, capture_output=True, text=True,
                                  timeout=max(args.timeout, task.get("timeout", 0)),
                                  env=isolated_env(workdir), cwd=workdir)
        except subprocess.TimeoutExpired:
            return None, "timeout"
        if done.returncode != 0:
            return None, classify_error(done.stderr or done.stdout) or f"exit {done.returncode}"
        try:
            report = json.loads(done.stdout)
        except ValueError:
            return None, "unparseable runner output"
        if report.get("error") and not report.get("calls"):
            # Failed before any tool call (provider error, rate limit): infrastructure, not a result.
            return None, classify_error(report["error"]) or "failed before any call"
        passed, why = task["check"](project, report)
        return {
            "task": task["name"], "tool": task["tool"], "passed": passed, "why": why,
            "answer": report.get("answer", "")[:300], "error": classify_error(report.get("error")),
            "error_text": (report.get("error") or "")[:300],
            "calls": call_rows(report), "tools_available": report.get("tools_available", []),
            "requests": report.get("requests", 0), "compactions": report.get("compactions", 0),
            "repeated_calls": repeated_calls(report), "input_tokens": report.get("input_tokens"),
            "output_tokens": report.get("output_tokens"), "latency_ms": report.get("latency_ms", 0),
            "sandboxed": command[0] == "sandbox-exec",
            "checkpoints": report.get("checkpoints", []), "plans": report.get("plans", []),
            "extra": task["measure"](project, report) if "measure" in task else {},
        }, None
    finally:
        shutil.rmtree(workdir, ignore_errors=True)


def repeated_calls(report):
    """Calls identical to an earlier call in the same task (redone work)."""
    seen, repeated = set(), 0
    for call in report.get("calls", []):
        key = (call["tool"], call.get("summary", ""))
        repeated += key in seen
        seen.add(key)
    return repeated


def _mean(values):
    values = [v for v in values if v is not None]
    return statistics.fmean(values) if values else None


def summarize(rows):
    """Per-task pass rates, per-tool call quality and overall efficiency."""
    by_task = collections.defaultdict(list)
    for row in rows:
        by_task[row["task"]].append(row)
    tasks = {name: {"tool": runs[0]["tool"], "runs": len(runs), "pass_rate": _mean([r["passed"] for r in runs]),
                    "mean_calls": _mean([len(r["calls"]) for r in runs]),
                    "mean_repeated_calls": _mean([r.get("repeated_calls", 0) for r in runs]),
                    "plan_used_rate": _mean([any(c["tool"] == "update_plan" for c in r["calls"]) for r in runs]),
                    "mean_compactions": _mean([r.get("compactions", 0) for r in runs]),
                    "mean_input_tokens": _mean([r.get("input_tokens") for r in runs]),
                    "failures": [r["why"] for r in runs if not r["passed"]][:3]}
             for name, runs in sorted(by_task.items())}
    calls = [c for row in rows for c in row["calls"]]
    tools = {}
    for tool in sorted({c["tool"] for c in calls}):
        mine = [c for c in calls if c["tool"] == tool]
        firsts = [c for c in mine if c["first"]]
        reasons = collections.Counter(c["reason"] for c in mine if c["reason"])
        tools[tool] = {"calls": len(mine), "first_try_success": _mean([c["reason"] is None for c in firsts]),
                       "wasted_share": _mean([c["reason"] is not None for c in mine]),
                       "reasons": dict(reasons.most_common())}
    return {
        "runs": len(rows), "task_pass_rate": _mean([r["passed"] for r in rows]),
        "mean_calls_per_task": _mean([len(r["calls"]) for r in rows]),
        "wasted_call_share": _mean([c["reason"] is not None for c in calls]),
        "first_try_success": _mean([c["reason"] is None for c in calls if c["first"]]),
        "mean_input_tokens": _mean([r["input_tokens"] for r in rows]),
        "mean_latency_s": _mean([r["latency_ms"] / 1000 for r in rows]),
        "tasks": tasks, "tools": tools,
    }


def fmt(value, percent=True):
    if value is None:
        return "n/a"
    return f"{value * 100:.0f}%" if percent else f"{value:.1f}"


def markdown(summary):
    s = summary["summary"]
    lines = [f"# Tool use: {summary.get('model')} ({summary.get('build_ref') or 'this checkout'})", "",
             f"Runs: {s['runs']}; infrastructure errors (excluded): {len(summary['errors'])}. Benchmark audit, not an automated check.", "",
             f"Task pass rate **{fmt(s['task_pass_rate'])}** · first-try tool success **{fmt(s['first_try_success'])}** · "
             f"wasted calls {fmt(s['wasted_call_share'])} · calls per task {fmt(s['mean_calls_per_task'], False)} · "
             f"input tokens per task {fmt(s['mean_input_tokens'], False)} · latency {fmt(s['mean_latency_s'], False)} s", "",
             "| Tool | Calls | First-try success | Wasted | Reasons |", "| --- | --- | --- | --- | --- |"]
    for tool, row in s["tools"].items():
        reasons = ", ".join(f"{k} {v}" for k, v in row["reasons"].items()) or "-"
        lines.append(f"| {tool} | {row['calls']} | {fmt(row['first_try_success'])} | {fmt(row['wasted_share'])} | {reasons} |")
    lines += ["", "| Task | Tool | Pass | Calls | Repeated calls | Compactions | Plan used | Input tokens | First failure |",
              "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for name, row in s["tasks"].items():
        lines.append(f"| {name} | {row['tool']} | {fmt(row['pass_rate'])} | {fmt(row['mean_calls'], False)} | "
                     f"{fmt(row.get('mean_repeated_calls'), False)} | {fmt(row.get('mean_compactions'), False)} | "
                     f"{fmt(row.get('plan_used_rate'))} | "
                     f"{fmt(row.get('mean_input_tokens'), False)} | {(row['failures'] or ['-'])[0][:80]} |")
    return "\n".join(lines) + "\n"


def run(args):
    cleanup = None
    if args.runner:
        runner = [os.path.abspath(args.runner)]
    elif args.build_ref:
        runner, cleanup = build_ref_runner(args.build_ref)
    else:
        runner = cargo_build(ROOT, os.path.join(ROOT, os.environ.get("CARGO_TARGET_DIR", "target")))
    tasks = [t for t in TASKS if not args.task or t["name"] in args.task]
    jobs = [(trial, task) for trial in range(args.trials) for task in tasks]
    rows, errors = [], []
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
            futures = {pool.submit(run_task, runner, task, args): (trial, task["name"]) for trial, task in jobs}
            for future in concurrent.futures.as_completed(futures):
                trial, name = futures[future]
                row, error = future.result()
                if error:
                    errors.append({"trial": trial, "task": name, "error": error})
                    print(f"trial {trial + 1} {name}: error {error}", flush=True)
                else:
                    rows.append(dict(row, trial=trial))
                    print(f"trial {trial + 1} {name}: {'pass' if row['passed'] else 'fail'}", flush=True)
    finally:
        if cleanup:
            cleanup()
    summary = {"suite": "tools", "model": args.model, "trials": args.trials, "build_ref": args.build_ref,
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
    parser = argparse.ArgumentParser(prog="python3 -m evals.tools")
    sub = parser.add_subparsers(dest="command", required=True)
    cmd = sub.add_parser("run")
    cmd.add_argument("--trials", type=int, default=3)
    cmd.add_argument("--model", default="longcat-2.5-preview-free")
    cmd.add_argument("--task", action="append", help="run only this task (repeatable)")
    cmd.add_argument("--jobs", type=int, default=2)
    cmd.add_argument("--max-turns", type=int, default=12)
    cmd.add_argument("--timeout", type=int, default=600)
    cmd.add_argument("--base-url")
    cmd.add_argument("--runner", help="prebuilt tool_eval binary")
    cmd.add_argument("--build-ref", help="measure another git ref's tools (e.g. main)")
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
