"""Long-horizon eval: big jobs that cross several context checkpoints, with and without notes.

    python3 -m evals.horizon run --trials 3 --model longcat-2.5-preview-free

Live-model benchmark audit; not an automated runtime check. Uses the tool-eval runner
(`crates/core/examples/tool_eval.rs`) and its isolation (scratch project, scratch HOME,
`sandbox-exec` on macOS). At every checkpoint it compares the notes the checkpoint carries
with the latest notes the agent saved.
"""

import argparse
import collections
import concurrent.futures
import datetime
import json
import os
import re
import sys

from evals.tools.__main__ import ROOT, _mean, cargo_build, fmt, run_task

from .tasks import TASKS

RESULTS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "results")


def _lines(notes):
    """Comparable (key, text, status) lines from a notes-log entry or a checkpoint block."""
    if isinstance(notes, dict):
        notes = notes.get("lines")
    if not isinstance(notes, list):
        return None
    return [(str(line.get("key", "")), str(line.get("text", "")), line.get("status"))
            for line in notes if isinstance(line, dict)]


def notes_persistence(row):
    """Per checkpoint: `kept` (carries the latest saved notes), `stale` (older notes),
    `dropped` (none although notes were saved), `changed` (something else), or `no_notes_yet`."""
    log = row.get("notes_log", [])
    verdicts = []
    for checkpoint in row.get("checkpoints", []):
        saved = [_lines(n) for n in log[:checkpoint["notes_calls_before"]] if n]
        carried = _lines(checkpoint.get("notes"))
        if not saved:
            verdicts.append("no_notes_yet")
        elif not carried:
            verdicts.append("dropped")
        elif carried == saved[-1]:
            verdicts.append("kept")
        elif carried in saved:
            verdicts.append("stale")
        else:
            verdicts.append("changed")
    return verdicts


def open_lines(row):
    """Task lines still open in the notes file at the end of the run."""
    return [line for line in row.get("final_notes") or [] if line.get("status") in ("pending", "in_progress")]


def would_stop(row):
    """The old guard stopped a run at its second warned streak: any turn with two warnings."""
    turns = row.get("turns") or []
    return any((turn.get("loop_warnings") or 0) >= 2 for turn in turns)


def failure_cause(row):
    """One coarse cause per failed run, from the turn errors and the check's reason."""
    if row["passed"]:
        return None
    errors = " ".join(str((turn.get("error") or "")) for turn in row.get("turns") or []) + str(row.get("error") or "")
    why = row.get("why", "")
    if "incomplete:" in errors or "max turns" in errors:
        return "turn_limit"
    if "Repeated unchanged" in errors:
        return "guard_stop"
    if errors.strip() and errors.strip() != "None":
        return "run_error"
    if "missing" in why or re.search(r"\b\d+ lines\b", why):
        return "unfinished"
    if "first line" in why or "functions" in why or "summary '" in why or why.startswith("line "):
        return "wrong_content"
    if "SUMMARY.md" in why or "conflicts.md" in why:
        return "wrong_result"
    if why.startswith("progress"):
        return "wrong_progress"
    return "wrong_final_answer"


def summarize(rows):
    by_task = collections.defaultdict(list)
    for row in rows:
        by_task[row["task"]].append(row)
    tasks = {}
    for name, runs in sorted(by_task.items()):
        verdicts = [v for r in runs for v in notes_persistence(r)]
        counted = [v for v in verdicts if v != "no_notes_yet"]
        extra = lambda key: _mean([r["extra"].get(key) for r in runs if key in r.get("extra", {})])  # noqa: E731
        tasks[name] = {
            "arm": runs[0].get("arm"), "runs": len(runs), "pass_rate": _mean([r["passed"] for r in runs]),
            "mean_compactions": _mean([r["compactions"] for r in runs]),
            "notes_used_rate": _mean([any(n for n in r.get("notes_log", [])) for r in runs]),
            "notes_kept_rate": _mean([v == "kept" for v in counted]),
            "checkpoints_with_notes": len(counted),
            "notes_verdicts": dict(collections.Counter(verdicts)),
            "open_at_end_rate": _mean([bool(open_lines(r)) for r in runs if r.get("final_notes")]),
            "progress_reported_rate": extra("progress_reported"),
            "early_detail_rate": extra("early_detail"), "count_recalled_rate": extra("count_recalled"),
            "mean_rereads": extra("rereads"), "mean_rule_kept_files": extra("rule_kept_files"),
            "mean_calls": _mean([len(r["calls"]) for r in runs]),
            "mean_input_tokens": _mean([r.get("input_tokens") for r in runs]),
            "mean_loop_warnings": _mean([r.get("loop_warnings", 0) for r in runs]),
            "old_guard_would_stop": sum(would_stop(r) for r in runs),
            "old_guard_would_stop_passed": sum(would_stop(r) and r["passed"] for r in runs),
            "causes": dict(collections.Counter(c for c in map(failure_cause, runs) if c)),
            "failures": [r["why"] for r in runs if not r["passed"]][:3],
        }
    return {"runs": len(rows), "task_pass_rate": _mean([r["passed"] for r in rows]), "tasks": tasks}


def markdown(summary):
    s = summary["summary"]
    lines = [f"# Long horizon: {summary.get('model')}", "",
             f"Runs: {s['runs']}; infrastructure errors (excluded): {len(summary['errors'])}. "
             "Benchmark audit, not an automated check.", "",
             "| Task | Pass | Compactions | Notes used | Notes kept at checkpoints | Open at end | Progress reported | "
             "Early detail recalled | Count recalled | Re-reads | Loop warnings | Old guard would stop (passed) | "
             "Calls | Input tokens | Failure causes |",
             "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for name, row in s["tasks"].items():
        kept = f"{fmt(row['notes_kept_rate'])} of {row['checkpoints_with_notes']}" if row["checkpoints_with_notes"] else "-"
        lines.append(f"| {name} | {fmt(row['pass_rate'])} | {fmt(row['mean_compactions'], False)} | "
                     f"{fmt(row['notes_used_rate'])} | {kept} | {fmt(row['open_at_end_rate'])} | "
                     f"{fmt(row['progress_reported_rate'])} | {fmt(row['early_detail_rate'])} | "
                     f"{fmt(row['count_recalled_rate'])} | {fmt(row['mean_rereads'], False)} | "
                     f"{fmt(row.get('mean_loop_warnings'), False)} | "
                     f"{row.get('old_guard_would_stop', '-')} ({row.get('old_guard_would_stop_passed', '-')}) | "
                     f"{fmt(row['mean_calls'], False)} | {fmt(row['mean_input_tokens'], False)} | "
                     f"{', '.join(f'{k} {v}' for k, v in (row.get('causes') or {}).items()) or '-'} |")
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
                row = dict(row, notes_persistence=notes_persistence(row))
            if error:
                errors.append({"trial": trial, "task": task["name"], "error": error,
                               **({"row": row} if row else {})})
                print(f"trial {trial + 1} {task['name']}: error {error}", flush=True)
            else:
                rows.append(dict(row, trial=trial, arm=task["arm"], notes_persistence=notes_persistence(row)))
                print(f"trial {trial + 1} {task['name']}: {'pass' if row['passed'] else 'fail'} "
                      f"({row['compactions']} compactions, notes {notes_persistence(row)})", flush=True)
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
