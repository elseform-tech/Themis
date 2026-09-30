from __future__ import annotations

import json
import os
import re
import shutil
import statistics
import subprocess
import threading
import time
import uuid
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
STATIC = Path(__file__).resolve().parent
STATE_DIR = ROOT / ".quality-dashboard"
HISTORY_PATH = STATE_DIR / "runs.json"
MAX_HISTORY = 60
IGNORED_DIRS = {".git", "target", "node_modules", "dist", "build", "coverage", ".quality-dashboard"}
SOURCE_ROOTS = ("crates", "web/src")
SOURCE_SUFFIXES = {".rs", ".ts", ".tsx", ".js", ".jsx", ".css", ".py", ".swift"}
TEST_FILE_RE = re.compile(r"(?:\.test\.(?:ts|tsx|js|jsx)$|(?:^|/)tests?/|(?:^|/)test_[^/]+\.py$)")
RUST_TEST_ATTR = re.compile(r"#\[\s*(?:tokio\s*::\s*)?test\b[^\]]*\]")
RUST_TEST_FN = re.compile(r"\bfn\s+([A-Za-z_]\w*)")
WEB_TEST_CALL = re.compile(r"(?<![.\w])(?:it|test)(?:\.(?:each|skip|only|todo))?\s*\(")
WEB_TEST_TITLE = re.compile(r"\s*(['\"`])((?:\\.|(?!\1)[\s\S])*?)\1")
RUST_TEST_TARGET = re.compile(r"Running (?:unittests\s+)?([^\s]+)")
ANSI_RE = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
AST_SUFFIXES = {".rs", ".ts", ".tsx", ".js", ".jsx", ".py"}
AST_ANALYZER_VERSION = "0.0.25"
RUN_COMMANDS = {
    "rust": ["cargo", "test", "--workspace", "--no-fail-fast"],
    "frontend": ["npm", "--prefix", "web", "test", "--", "--reporter=verbose"],
    "dashboard": ["python3", "-m", "unittest", "quality_dashboard.test_dashboard"],
}

_STATE_LOCK = threading.RLock()
_ACTIVE_RUN: dict | None = None
_RUN_HISTORY: list[dict] = []
_AST_CACHE_LOCK = threading.Lock()
_AST_CACHE: dict[str, tuple[int, int, dict]] = {}


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def read_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""


def run_git(*args: str, timeout: float = 4) -> str:
    try:
        result = subprocess.run(
            ["git", *args], cwd=ROOT, capture_output=True, text=True,
            timeout=timeout, check=False,
        )
        return result.stdout if result.returncode == 0 else ""
    except (OSError, subprocess.TimeoutExpired):
        return ""


def tracked_source_files() -> list[Path]:
    files: list[Path] = []
    for root_name in SOURCE_ROOTS:
        root = ROOT / root_name
        if not root.exists():
            continue
        for path in root.rglob("*"):
            if not path.is_file() or path.suffix not in SOURCE_SUFFIXES:
                continue
            if any(part in IGNORED_DIRS for part in path.parts):
                continue
            files.append(path)
    return sorted(set(files))


def line_count(text: str) -> int:
    return len(text.splitlines())


def is_test_file(path: Path) -> bool:
    rel = path.relative_to(ROOT).as_posix()
    return bool(TEST_FILE_RE.search(rel)) or "_e2e." in path.name.lower()


def source_metrics() -> dict:
    language_ext = {".rs": "Rust", ".ts": "TypeScript", ".tsx": "TSX", ".js": "JavaScript", ".jsx": "JSX", ".css": "CSS", ".py": "Python", ".swift": "Swift"}
    files = tracked_source_files()
    total = tests = production = 0
    by_language: dict[str, dict[str, int]] = {}
    for path in files:
        count = line_count(read_text(path))
        test_file = is_test_file(path)
        total += count
        tests += count if test_file else 0
        production += count if not test_file else 0
        key = language_ext.get(path.suffix, path.suffix)
        row = by_language.setdefault(key, {"files": 0, "loc": 0})
        row["files"] += 1
        row["loc"] += count
    return {"files": len(files), "loc": total, "productionLoc": production, "testLoc": tests, "byLanguage": by_language}


def _test_names(path: Path, text: str) -> list[str]:
    if path.suffix == ".rs":
        names = []
        for match in RUST_TEST_ATTR.finditer(text):
            fn = RUST_TEST_FN.search(text, match.end(), min(len(text), match.end() + 500))
            if fn:
                names.append(fn.group(1))
        return names
    if path.suffix == ".py":
        return re.findall(r"^\s+def\s+(test_[A-Za-z_]\w*)\s*\(", text, re.MULTILINE)
    names = []
    for match in WEB_TEST_CALL.finditer(text):
        title = WEB_TEST_TITLE.match(text, match.end())
        names.append(title.group(2).strip() if title else f"Parameterized test {len(names) + 1} (title resolved at runtime)")
    return names


def test_inventory() -> dict:
    suites: list[dict] = []
    counts = {"unit": 0, "integration": 0, "e2e": 0}
    tooling_counts = {"unit": 0, "integration": 0, "e2e": 0}
    candidates = [path for path in (ROOT / "crates").rglob("*.rs") if path.is_file()]
    candidates += [path for path in (ROOT / "web/src").rglob("*.test.*") if path.is_file()]
    candidates += [path for path in (ROOT / "quality_dashboard").glob("test_*.py") if path.is_file()]
    for path in sorted(candidates):
        names = _test_names(path, read_text(path))
        if not names:
            continue
        rel = path.relative_to(ROOT).as_posix()
        if "e2e" in path.name.lower():
            kind = "e2e"
        elif "/tests/" in f"/{rel}/":
            kind = "integration"
        else:
            kind = "unit"
        framework = "Rust test harness" if path.suffix == ".rs" else "Python unittest" if path.suffix == ".py" else "Vitest"
        scope = "tooling" if rel.startswith("quality_dashboard/") else "product"
        suites.append({"path": rel, "framework": framework, "kind": kind, "scope": scope, "tests": len(names), "names": names})
        target_counts = tooling_counts if scope == "tooling" else counts
        target_counts[kind] += len(names)
    total = sum(counts.values())
    tooling_total = sum(tooling_counts.values())
    return {
        "counts": counts,
        "total": total,
        "overallTotal": total,
        "toolingCounts": tooling_counts,
        "toolingTotal": tooling_total,
        "discoveredTotal": total + tooling_total,
        "overallDiscoveredTotal": total + tooling_total,
        "suites": suites,
    }


def _complexity_source_files() -> list[Path]:
    return [
        path
        for path in tracked_source_files()
        if path.suffix in AST_SUFFIXES
        and not is_test_file(path)
        and path.relative_to(ROOT).as_posix().startswith(("crates/", "web/src/"))
    ]


def _metric_number(value: object, digits: int = 2) -> float:
    if not isinstance(value, (int, float)):
        return 0.0
    return round(float(value), digits)


def _ast_complexity_row(path: str, analysis: dict) -> dict:
    metrics = analysis.get("metrics", {})
    root_spaces = analysis.get("spaces")
    spaces = list(root_spaces) if isinstance(root_spaces, list) else []
    function_indexes = []
    while spaces:
        space = spaces.pop()
        if not isinstance(space, dict):
            continue
        nested_spaces = space.get("spaces")
        if isinstance(nested_spaces, list):
            spaces.extend(nested_spaces)
        if space.get("kind") in {"function", "method"}:
            function_indexes.append(
                _metric_number(
                    space.get("metrics", {}).get("mi", {}).get("mi_visual_studio"), 0,
                )
            )
    return {
        "path": Path(path).as_posix(),
        "loc": int(_metric_number(metrics.get("loc", {}).get("sloc"), 0)),
        "functions": int(_metric_number(metrics.get("nom", {}).get("functions"), 0)),
        "cyclomatic": _metric_number(metrics.get("cyclomatic", {}).get("average")),
        "cognitive": _metric_number(metrics.get("cognitive", {}).get("average")),
        "maintainability": round(statistics.mean(function_indexes), 1) if function_indexes else _metric_number(metrics.get("mi", {}).get("mi_visual_studio"), 1),
        "maintainabilityTotal": sum(function_indexes),
        "maintainabilitySamples": len(function_indexes),
    }


def _parse_ast_output(output: str) -> list[dict]:
    decoder = json.JSONDecoder()
    records = []
    offset = 0
    while offset < len(output):
        while offset < len(output) and output[offset].isspace():
            offset += 1
        if offset == len(output):
            break
        record, offset = decoder.raw_decode(output, offset)
        if not isinstance(record, dict):
            raise ValueError("AST analyzer returned a non-object record")
        records.append(record)
    return records


def _run_ast_analysis(paths: list[Path], executable: str) -> tuple[dict[str, dict], str | None]:
    command = [executable, "-m", "-O", "json", "-j", "2"]
    expected = {path.relative_to(ROOT).as_posix() for path in paths}
    for relative in sorted(expected):
        command.extend(("-p", relative))
    try:
        result = subprocess.run(
            command, cwd=ROOT, capture_output=True, text=True, timeout=60, check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        return {}, str(error)[:240]
    if result.returncode != 0:
        return {}, (result.stderr.strip() or f"Analyzer exited with code {result.returncode}")[:240]
    try:
        records = _parse_ast_output(result.stdout)
        rows = {
            Path(record["name"]).as_posix(): _ast_complexity_row(record["name"], record)
            for record in records
            if isinstance(record.get("name"), str)
        }
    except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        return {}, f"Could not read AST analyzer output: {error}"[:240]
    missing = expected - rows.keys()
    if missing:
        return {}, f"AST analyzer returned no metrics for {len(missing)} source files"
    return rows, None


def _ast_metrics_unavailable(error: str) -> dict:
    return {
        "available": False,
        "method": f"Tree-sitter AST metrics via rust-code-analysis {AST_ANALYZER_VERSION}.",
        "error": error,
        "filesAnalyzed": 0,
        "averageCyclomatic": None,
        "averageCognitive": None,
        "averageMaintainability": None,
        "files": [],
    }


def complexity_metrics() -> dict:
    paths = _complexity_source_files()
    active_paths = {path.relative_to(ROOT).as_posix(): path for path in paths}
    with _AST_CACHE_LOCK:
        for cached_path in _AST_CACHE.keys() - active_paths.keys():
            del _AST_CACHE[cached_path]
        changed = []
        fingerprints = {}
        for relative, path in active_paths.items():
            try:
                stat = path.stat()
            except OSError as error:
                return _ast_metrics_unavailable(str(error)[:240])
            fingerprint = (stat.st_mtime_ns, stat.st_size)
            fingerprints[relative] = fingerprint
            cached = _AST_CACHE.get(relative)
            if cached is None or cached[:2] != fingerprint:
                changed.append(path)
        if changed:
            configured = os.environ.get("RUST_CODE_ANALYSIS_CLI", "rust-code-analysis-cli")
            executable = shutil.which(configured)
            if executable is None:
                return _ast_metrics_unavailable(
                    "Install the analyzer with: cargo install rust-code-analysis-cli "
                    f"--version {AST_ANALYZER_VERSION} --locked"
                )
            rows, error = _run_ast_analysis(changed, executable)
            if error:
                return _ast_metrics_unavailable(error)
            for relative, row in rows.items():
                mtime_ns, size = fingerprints[relative]
                _AST_CACHE[relative] = (mtime_ns, size, row)
        rows = [_AST_CACHE[relative][2] for relative in active_paths]
    return {
        "available": True,
        "method": (
            f"Tree-sitter AST metrics via rust-code-analysis {AST_ANALYZER_VERSION}; "
            "Visual Studio maintainability index, mean across callable AST nodes. Files without functions use module index in the table only."
        ),
        "filesAnalyzed": len(rows),
        "averageCyclomatic": round(statistics.mean(row["cyclomatic"] for row in rows), 2) if rows else 0,
        "averageCognitive": round(statistics.mean(row["cognitive"] for row in rows), 2) if rows else 0,
        "averageMaintainability": round(
            sum(row["maintainabilityTotal"] for row in rows) / sum(row["maintainabilitySamples"] for row in rows), 1,
        ) if sum(row["maintainabilitySamples"] for row in rows) else 0,
        "files": sorted(rows, key=lambda row: (row["maintainability"], -row["cognitive"])),
    }


def parse_lcov(text: str) -> dict | None:
    lines_found = lines_hit = 0
    for line in text.splitlines():
        try:
            if line.startswith("LF:"):
                lines_found += int(line[3:] or 0)
            elif line.startswith("LH:"):
                lines_hit += int(line[3:] or 0)
        except ValueError:
            continue
    if not lines_found:
        return None
    return {"percent": round(lines_hit * 100 / lines_found, 1), "hit": lines_hit, "found": lines_found}


def coverage_metrics() -> dict:
    candidates = [
        ("Rust", ROOT / "coverage/lcov.info"),
        ("Frontend", ROOT / "web/coverage/lcov.info"),
    ]
    reports = []
    for scope, path in candidates:
        if not path.is_file():
            continue
        report = parse_lcov(read_text(path))
        if report:
            reports.append({"scope": scope, **report, "path": path.relative_to(ROOT).as_posix()})
    return {"available": bool(reports), "reports": reports}


def git_metrics() -> dict:
    branch = run_git("branch", "--show-current").strip() or "unknown"
    commit = run_git("rev-parse", "--short", "HEAD").strip() or "unknown"
    raw_status = run_git("status", "--porcelain=v1", "--untracked-files=all")
    statuses = {}
    for line in raw_status.splitlines():
        if len(line) < 4:
            continue
        state, path = line[:2], line[3:]
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        statuses[path] = state
    numstat = {}
    for line in run_git("diff", "--numstat", "HEAD").splitlines():
        parts = line.split("\t", 2)
        if len(parts) == 3 and parts[0].isdigit() and parts[1].isdigit():
            numstat[parts[2]] = (int(parts[0]), int(parts[1]))
    changes = []
    for path, state in sorted(statuses.items()):
        added, removed = numstat.get(path, (0, 0))
        if state == "??":
            added = line_count(read_text(ROOT / path))
        changes.append({"path": path, "status": state, "added": added, "removed": removed})
    recent = []
    for line in run_git("log", "-n", "30", "--format=%h%x09%as%x09%s").splitlines():
        parts = line.split("\t", 2)
        if len(parts) == 3:
            recent.append({"commit": parts[0], "date": parts[1], "subject": parts[2]})
    product_paths = ("--", "crates", "web/src")
    churn_lines = run_git("log", "--since=30 days ago", "--numstat", "--format=", *product_paths).splitlines()
    churn_added = churn_removed = 0
    for line in churn_lines:
        parts = line.split("\t", 2)
        if len(parts) == 3:
            churn_added += int(parts[0]) if parts[0].isdigit() else 0
            churn_removed += int(parts[1]) if parts[1].isdigit() else 0
    commits = run_git("rev-list", "--count", "--since=30 days ago", "HEAD", *product_paths).strip()
    return {
        "branch": branch, "commit": commit, "clean": not statuses,
        "changes": changes,
        "workingChurn": {"added": sum(row["added"] for row in changes), "removed": sum(row["removed"] for row in changes)},
        "monthChurn": {"commits": int(commits) if commits.isdigit() else 0, "added": churn_added, "removed": churn_removed},
        "recentCommits": recent,
    }


def parse_test_totals(suite: str, output: str) -> dict:
    if suite == "rust":
        matches = re.findall(r"test result: (?:ok|FAILED)\.\s*(\d+) passed;\s*(\d+) failed;\s*(\d+) ignored", output)
        return {"passed": sum(int(row[0]) for row in matches), "failed": sum(int(row[1]) for row in matches), "skipped": sum(int(row[2]) for row in matches)}
    if suite == "dashboard":
        match = re.search(r"Ran\s+(\d+)\s+tests?", output)
        if not match:
            return {"passed": None, "failed": None, "skipped": None}
        total = int(match.group(1))
        failure = re.search(r"FAILED\s*\(([^)]*)\)", output)
        summary = failure.group(1) if failure else ""
        failed = sum(int(value) for value in re.findall(r"(?:failures|errors)=([\d]+)", summary))
        skipped_match = re.search(r"skipped=(\d+)", summary)
        skipped = int(skipped_match.group(1)) if skipped_match else 0
        return {"passed": max(0, total - failed - skipped), "failed": failed, "skipped": skipped}
    match = re.search(r"^\s*Tests\s+(.+?)\s*\(([\d,]+)\)\s*$", output, re.MULTILINE)
    if not match:
        return {"passed": None, "failed": None, "skipped": None}
    totals = {"passed": 0, "failed": 0, "skipped": 0}
    label_map = {"passed": "passed", "failed": "failed", "skipped": "skipped", "todo": "skipped"}
    for value, label in re.findall(r"([\d,]+)\s+(passed|failed|skipped|todo)", match.group(1)):
        totals[label_map[label]] += int(value.replace(",", ""))
    return totals


def parse_case_event(suite: str, line: str, rust_path: str | None = None, frontend_paths: set[str] | None = None, rust_modules: dict[str, list[str]] | None = None, rust_test_names: dict[str, list[str]] | None = None) -> dict[str, str | None] | None:
    line = ANSI_RE.sub("", line).strip()
    if suite == "rust":
        match = re.match(r"test\s+(.+?)\s+\.\.\.\s+(ok|FAILED|ignored|measured)$", line)
        if not match:
            return None
        status = {"ok": "passed", "FAILED": "failed", "ignored": "skipped", "measured": "skipped"}[match.group(2)]
        path = rust_path if rust_path and Path(rust_path).name not in {"lib.rs", "main.rs"} else None
        if path is None:
            for module in match.group(1).split("::"):
                candidates = (rust_modules or {}).get(module, [])
                if len(candidates) == 1:
                    path = candidates[0]
                    break
        if path is None:
            candidates = (rust_test_names or {}).get(match.group(1).split("::")[-1], [])
            if len(candidates) == 1:
                path = candidates[0]
        return {"path": path, "name": match.group(1), "status": status}
    if suite == "frontend":
        match = re.match(r"([✓✔×✗])\s+(.+?)(?:\s+\(?[\d.]+ms\)?)?$", line)
        if not match or " > " not in match.group(2) or ".test." not in match.group(2):
            return None
        status = "failed" if match.group(1) in {"×", "✗"} else "passed"
        description = match.group(2).strip()
        reported_path = description.split(" > ", 1)[0].replace("\\", "/")
        test_path = next(
            (
                path
                for path in frontend_paths or ()
                if reported_path.endswith((path, path.removeprefix("web/")))
            ),
            None,
        )
        return {"path": test_path, "name": description, "status": status}
    return None


def rust_test_target(line: str, source_by_name: dict[str, str]) -> tuple[bool, str | None]:
    match = RUST_TEST_TARGET.search(line)
    return (True, source_by_name.get(Path(match.group(1)).name)) if match else (False, None)


def load_history() -> list[dict]:
    try:
        data = json.loads(read_text(HISTORY_PATH))
        return data[-MAX_HISTORY:] if isinstance(data, list) else []
    except (ValueError, OSError):
        return []


def save_history() -> None:
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    tmp = HISTORY_PATH.with_suffix(".tmp")
    tmp.write_text(json.dumps(_RUN_HISTORY[-MAX_HISTORY:], indent=2), encoding="utf-8")
    tmp.replace(HISTORY_PATH)


def _run_suites(run_id: str, selected: list[str]) -> None:
    global _ACTIVE_RUN
    results = []
    for suite in selected:
        started = time.monotonic()
        with _STATE_LOCK:
            if _ACTIVE_RUN and _ACTIVE_RUN["id"] == run_id:
                _ACTIVE_RUN["currentSuite"] = suite
                _ACTIVE_RUN["checks"].append({"suite": suite, "status": "running", "startedAt": now_iso(), "passed": 0, "failed": 0, "skipped": 0})
        env = os.environ.copy()
        env.update({"CI": "1", "NO_COLOR": "1", "CARGO_TERM_COLOR": "never", "FORCE_COLOR": "0"})
        output_tail: list[str] = []
        live_counts = {"passed": 0, "failed": 0, "skipped": 0}
        file_results: dict[str, dict[str, int]] = {}
        case_results: list[dict[str, str | None]] = []
        inventory = test_inventory()["suites"]
        source_by_name = {Path(row["path"]).name: row["path"] for row in inventory if row["framework"] == "Rust test harness"}
        frontend_paths = {row["path"] for row in inventory if row["framework"] == "Vitest"}
        rust_modules: dict[str, list[str]] = {}
        rust_test_names: dict[str, list[str]] = {}
        for row in inventory:
            if row["framework"] == "Rust test harness":
                module = Path(row["path"]).stem
                rust_modules.setdefault(module, []).append(row["path"])
                for name in row["names"]:
                    rust_test_names.setdefault(name, []).append(row["path"])
        current_test_file = None
        try:
            process = subprocess.Popen(
                RUN_COMMANDS[suite], cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                text=True, bufsize=1, env=env,
            )
            assert process.stdout is not None
            for line in process.stdout:
                output_tail.append(line)
                if len(output_tail) > 5000:
                    del output_tail[:1000]
                has_test_target, test_target = rust_test_target(line, source_by_name) if suite == "rust" else (False, None)
                if has_test_target:
                    current_test_file = test_target
                event = parse_case_event(suite, line, current_test_file, frontend_paths, rust_modules, rust_test_names)
                if event:
                    result_key = event["status"]
                    live_counts[result_key] += 1
                    case_results.append(event)
                    if event["path"]:
                        row = file_results.setdefault(event["path"], {"passed": 0, "failed": 0, "skipped": 0})
                        row[result_key] += 1
                    with _STATE_LOCK:
                        if _ACTIVE_RUN and _ACTIVE_RUN["id"] == run_id and _ACTIVE_RUN["checks"]:
                            _ACTIVE_RUN["checks"][-1].update(live_counts)
                            _ACTIVE_RUN["checks"][-1]["completed"] = sum(live_counts.values())
                            _ACTIVE_RUN["checks"][-1]["caseResults"] = case_results.copy()
            returncode = process.wait()
            output = "".join(output_tail)
            totals = parse_test_totals(suite, output)
            status = "passed" if returncode == 0 else "failed"
            detail = None if returncode == 0 else next((line.strip() for line in reversed(output_tail) if line.strip().startswith(("error:", "Error:", "FAIL"))), f"Exited with code {returncode}")[:240]
        except OSError as error:
            returncode, totals, status, detail = 127, {"passed": None, "failed": None, "skipped": None}, "failed", str(error)[:240]
        result = {
            "suite": suite, "status": status, "exitCode": returncode,
            **totals, "durationSeconds": round(time.monotonic() - started, 1),
            "finishedAt": now_iso(), "detail": detail,
        }
        if file_results:
            result["fileResults"] = file_results
        if case_results:
            result["caseResults"] = case_results
        results.append(result)
        with _STATE_LOCK:
            if _ACTIVE_RUN and _ACTIVE_RUN["id"] == run_id:
                _ACTIVE_RUN["checks"][-1] = {**_ACTIVE_RUN["checks"][-1], **result}
    finished = now_iso()
    run = {"id": run_id, "status": "passed" if all(row["status"] == "passed" for row in results) else "failed", "startedAt": None, "finishedAt": finished, "checks": results}
    with _STATE_LOCK:
        if _ACTIVE_RUN and _ACTIVE_RUN["id"] == run_id:
            run["startedAt"] = _ACTIVE_RUN["startedAt"]
            run["commit"] = _ACTIVE_RUN["commit"]
        _RUN_HISTORY.append(run)
        del _RUN_HISTORY[:-MAX_HISTORY]
        _ACTIVE_RUN = None
        try:
            save_history()
        except OSError:
            pass


def dashboard_data() -> dict:
    with _STATE_LOCK:
        active = json.loads(json.dumps(_ACTIVE_RUN)) if _ACTIVE_RUN else None
        history = json.loads(json.dumps(_RUN_HISTORY[-MAX_HISTORY:]))
    inventory = test_inventory()
    return {
        "updatedAt": now_iso(),
        "repo": git_metrics(),
        "source": source_metrics(),
        "tests": inventory,
        "complexity": complexity_metrics(),
        "coverage": coverage_metrics(),
        "activeRun": active,
        "runs": history,
    }


class DashboardHandler(BaseHTTPRequestHandler):
    server_version = "ThemisQualityDashboard/1.0"

    def log_message(self, format: str, *args) -> None:
        print(f"[quality-dashboard] {self.address_string()} {format % args}")

    def _send(self, body: bytes, content_type: str, status: int = 200) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Content-Security-Policy", "default-src 'self'; connect-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self'; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'")
        self.send_header("X-Frame-Options", "DENY")
        self.send_header("Referrer-Policy", "no-referrer")
        self.end_headers()
        self.wfile.write(body)

    def _json(self, data: dict, status: int = 200) -> None:
        self._send(json.dumps(data).encode("utf-8"), "application/json; charset=utf-8", status)

    def do_GET(self) -> None:
        path = unquote(urlsplit(self.path).path)
        if path == "/api/metrics":
            self._json(dashboard_data())
            return
        relative = "index.html" if path == "/" else path.lstrip("/")
        target = (STATIC / relative).resolve()
        if STATIC.resolve() not in target.parents or not target.is_file():
            self._json({"error": "Not found"}, 404)
            return
        content_type = "text/html; charset=utf-8" if target.suffix == ".html" else "text/css; charset=utf-8" if target.suffix == ".css" else "text/javascript; charset=utf-8"
        self._send(target.read_bytes(), content_type)

    def do_POST(self) -> None:
        global _ACTIVE_RUN
        if urlsplit(self.path).path != "/api/run":
            self._json({"error": "Not found"}, 404)
            return
        host = self.headers.get("Host", "")
        origin = self.headers.get("Origin")
        if host not in {"127.0.0.1:4178", "localhost:4178"} or origin not in {None, "http://127.0.0.1:4178", "http://localhost:4178"}:
            self._json({"error": "Local requests only"}, 403)
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if length < 0 or length > 1024:
                self._json({"error": "Request too large"}, 413)
                return
            data = json.loads(self.rfile.read(length) or b"{}")
        except (ValueError, json.JSONDecodeError):
            self._json({"error": "Invalid JSON"}, 400)
            return
        suite = data.get("suite", "all") if isinstance(data, dict) else "all"
        selected = list(RUN_COMMANDS) if suite == "all" else [suite] if suite in RUN_COMMANDS else []
        if not selected:
            self._json({"error": "Choose rust, frontend, dashboard, or all"}, 400)
            return
        run_id = uuid.uuid4().hex[:12]
        with _STATE_LOCK:
            if _ACTIVE_RUN:
                self._json({"error": "A test run is already active"}, 409)
                return
            _ACTIVE_RUN = {"id": run_id, "status": "running", "startedAt": now_iso(), "commit": run_git("rev-parse", "--short", "HEAD").strip(), "currentSuite": None, "checks": []}
        threading.Thread(target=_run_suites, args=(run_id, selected), daemon=True).start()
        self._json({"id": run_id, "status": "started"}, 202)


def main() -> None:
    global _RUN_HISTORY
    _RUN_HISTORY = load_history()
    server = ThreadingHTTPServer(("127.0.0.1", 4178), DashboardHandler)
    print("Quality dashboard: http://127.0.0.1:4178")
    print("Listening on localhost only; press Ctrl-C to stop.")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
