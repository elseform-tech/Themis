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
from .report import INFRASTRUCTURE_CLASSES, aggregate, classify_error, markdown, trial_summary
from .validate import MAX_CITATIONS, context_location, validate

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
    """Builds the runner; `--cfg eval_recall` registers the `recall` tool when the tree has one."""
    env = dict(os.environ, CARGO_TARGET_DIR=target)
    if os.path.isfile(os.path.join(cwd, "crates/core/src/tools/recall.rs")):
        env["RUSTFLAGS"] = (env.get("RUSTFLAGS", "") + " --cfg eval_recall").strip()
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


def stored_answer(entry):
    """The answer text, or None when the request itself failed (provider/transport error)."""
    if not entry:
        return None
    answer = entry.get("answer") or ""
    if answer:
        return answer
    error = entry.get("error") or ""
    kind = error if error.startswith(INFRASTRUCTURE_CLASSES) else classify_error(error)
    return None if not kind or kind.startswith(INFRASTRUCTURE_CLASSES) else ""


def process_run(case, report, workdir):
    pool = case.get("tokens", [])
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
        entry = dict(answers.get(probe["id"], {}))
        entry["error"] = classify_error(entry.get("error"))  # redacted: class and HTTP status only
        answer = stored_answer(entry)
        decoy = probe["category"] == "decoy"
        cite = {"present": False, "valid": False, "count": 0, "over_cap": False, "cited": []}
        if not decoy and answer:
            cite = validate(evidence, probe, answer)
        tools = entry.get("tools", [])
        rows.append({
            "case": case["name"], "id": probe["id"], "category": probe["category"], "domain": case["kind"],
            "in_context": None if decoy else context_location(messages, probe),
            "verdict": grade(probe, answer, pool), "answer": answer or "", "error": entry.get("error"),
            "cite_present": cite["present"], "cite_valid": cite["valid"], "cited": cite["cited"],
            "cite_count": cite.get("count", 0), "cite_over_cap": cite.get("over_cap", False),
            "refused_paths": entry.get("refused_paths", []),
            "tools": tools, "tool_calls": sum(t != "update_plan" for t in tools),
            "denied": entry.get("denied", 0),
            "executed_tool_calls": max(0, sum(t != "update_plan" for t in tools) - entry.get("denied", 0)), "used_recall": "recall" in tools,
            "requests": entry.get("requests", 0), "input_tokens": entry.get("input_tokens"),
            "output_tokens": entry.get("output_tokens"), "latency_ms": entry.get("latency_ms", 0),
        })
    report["chunks"] = [dict(c, error=classify_error(c.get("error"))) for c in report["chunks"]]
    report["recall_available"] = "recall" in available
    report["tools_available"] = available
    return rows


def scratch_inputs(case, workdir):
    """Copies the fixture inputs the runner needs; probes.json is stripped to id and question."""
    inputs = os.path.join(workdir, "inputs")
    os.makedirs(inputs)
    for name in ("case.json", "transcript.jsonl"):
        shutil.copy(os.path.join(case["directory"], name), inputs)
    with open(os.path.join(inputs, "probes.json"), "w", encoding="utf-8") as handle:
        json.dump([{"id": p["id"], "question": p["question"]} for p in case["probes"]], handle)
    return inputs


def isolated_env(workdir):
    """A minimal environment: scratch HOME/TMP (no real home or app data), PATH and the provider key."""
    home = os.path.join(workdir, "home")
    tmp = os.path.join(workdir, "tmp")
    os.makedirs(home)
    os.makedirs(tmp)
    keep = ("PATH", "OPENCODE_KEY", "SYSTEMROOT", "SYSTEMDRIVE", "SSL_CERT_FILE", "SSL_CERT_DIR")
    env = {k: os.environ[k] for k in keep if k in os.environ}
    env.update(HOME=home, USERPROFILE=home, APPDATA=home, LOCALAPPDATA=home, XDG_DATA_HOME=home,
               XDG_CONFIG_HOME=home, TMPDIR=tmp, TEMP=tmp, TMP=tmp)
    return env


def sandbox_wrapper(runner, workdir, mode):
    """macOS `sandbox-exec` prefix that blocks reads/writes under /Users and /Volumes except the
    runner binary and the scratch directory. Network is NOT restricted. Returns [] when unavailable."""
    if mode == "off" or sys.platform != "darwin" or not shutil.which("sandbox-exec") or not os.path.isfile(runner[0]):
        return []
    real = os.path.realpath
    profile = os.path.join(workdir, "sandbox.sb")
    with open(profile, "w", encoding="utf-8") as handle:
        handle.write('(version 1)\n(allow default)\n'
                     '(deny file-read* (subpath "/Users") (subpath "/Volumes") (subpath "/private/etc/ssh"))\n'
                     '(deny file-write* (subpath "/Users") (subpath "/Volumes"))\n'
                     f'(allow file-read* (literal "{real(runner[0])}") (subpath "{real(workdir)}"))\n')
    return ["sandbox-exec", "-f", profile]


def run_once(runner, case, args):
    workdir = tempfile.mkdtemp(prefix="longform-run-")  # system temp: outside the repo and ~/Documents
    try:
        inputs = scratch_inputs(case, workdir)
        command = runner + ["--case", inputs, "--workdir", workdir, "--budget", str(args.budget),
                            "--recent", str(args.recent), "--max-calls", str(args.max_calls)]
        if args.model:
            command += ["--model", args.model]
        if args.base_url:
            command += ["--base-url", args.base_url]
        command = sandbox_wrapper(runner, workdir, getattr(args, "sandbox", "auto")) + command
        try:
            done = subprocess.run(command, capture_output=True, text=True, timeout=args.timeout,
                                  env=isolated_env(workdir))
        except subprocess.TimeoutExpired:
            return None, "timeout"
        if done.returncode != 0:
            # Only the error class is kept: stderr may carry provider response bodies.
            return None, classify_error(done.stderr or done.stdout) or f"exit {done.returncode}"
        try:
            report = json.loads(done.stdout)
        except ValueError:
            return None, "unparseable runner output"
        report["rows"] = process_run(case, report, workdir)
        report["sandboxed"] = command[0] == "sandbox-exec"
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
        "max_tool_calls": args.max_calls, "sandboxed": any(r.get("sandboxed") for r in results.values()), "failed_runs": failures, "build_ref": args.build_ref,
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


def regrade(args):
    """Re-grades stored answers offline with the current grader and fixtures (citations are kept)."""
    with open(args.result, encoding="utf-8") as handle:
        old = json.load(handle)
    cases = {c["name"]: c for c in load_cases("longform")}
    results = {}
    for item in old["details"]:
        case = cases[item["case"]]
        probes = {p["id"]: p for p in case["probes"]}
        rows = []
        for row in item["rows"]:
            row = dict(row)
            row["error"] = row["error"] if (row["error"] or "").split(":")[0] in (
                "timeout", "provider_request", "provider_response", "tool_loop", "stopped", "turn_limit", "other"
            ) else classify_error(row["error"])
            row["verdict"] = grade(probes[row["id"]], stored_answer(
                {"answer": row["answer"], "error": row["error"]}), case.get("tokens", []))
            row["executed_tool_calls"] = max(0, row["tool_calls"] - row["denied"])
            if row["cite_present"] and "cite_count" not in row:
                row["cite_count"] = len(row["cited"])
                row["cite_over_cap"] = len(row["cited"]) > MAX_CITATIONS
                row["cite_valid"] = row["cite_valid"] and not row["cite_over_cap"]
            rows.append(row)
        results[(item["trial"], item["case"])] = {
            "chunks": item["chunks"], "rows": rows, "recall_available": "recall" in (item.get("tools_available") or []),
            "tools_available": item.get("tools_available")}
    summary = build_summary([cases[n] for n in old["cases"]], results, old["trials_requested"])
    for key in ("suite", "model", "budget", "recent_messages", "max_tool_calls", "failed_runs", "build_ref",
                "created", "git_commit", "sandboxed"):
        summary[key] = old.get(key)
    summary["note"] = ("Live-model benchmark audit; not an automated runtime check. Offline regrade of the stored "
                       "answers with the final grader (strict abstention, hedged answers, citation cap of "
                       f"{MAX_CITATIONS}); model output is unchanged. The first run used the earlier harness: "
                       "unsandboxed shell, git tool allowed, raw error text stored.")
    out = args.out or args.result
    with open(out, "w", encoding="utf-8") as handle:
        json.dump(summary, handle, indent=2, ensure_ascii=False)
        handle.write("\n")
    print(markdown(summary))
    return 0


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
    cmd.add_argument("--sandbox", choices=("auto", "off"), default="auto",
                     help="wrap the runner in macOS sandbox-exec (file reads/writes under /Users blocked)")
    cmd.add_argument("--base-url", default=None, help="point the provider at a mock server")
    cmd.add_argument("--jobs", type=int, default=3)
    cmd.add_argument("--timeout", type=int, default=3600)
    group = cmd.add_mutually_exclusive_group()
    group.add_argument("--runner", default=None, help="prebuilt longform_eval binary (skips the build)")
    group.add_argument("--build-ref", default=None, help="git ref to build the tools/runtime from (eval entry points overlaid)")
    cmd.add_argument("--out", default=None)
    again = sub.add_parser("report", help="print the markdown summary of a stored result")
    again.add_argument("result")
    redo = sub.add_parser("regrade", help="re-grade a stored result offline and rewrite it")
    redo.add_argument("result")
    redo.add_argument("--out", default=None)
    args = parser.parse_args(argv)
    return {"report": show, "regrade": regrade}.get(args.command, run)(args)


if __name__ == "__main__":
    sys.exit(main())
