"""Re-runs the tests a CI `cargo test` run failed, once, and reports the ones that pass as flaky.

usage: python3 scripts/ci_rerun_flaky.py <cargo-test-log>

Exit 0 when every failed test passes on its re-run (each is reported as FLAKY in the job
summary and as a warning on the pull request); exit 1 when any fails again, when the log names
no failed test (a build error, say), or when more than MAX_RERUNS failed (a real breakage, not
a flake). Each test runs again from the binary cargo already built, in its crate's folder.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MAX_RERUNS = 5
RERUN_TIMEOUT_S = 900
_ANSI = re.compile(r"\x1b\[[0-9;]*m")
_RUNNING = re.compile(r"^\s*Running\s+(?P<what>\S+(?:\s+\S+)?)\s+\((?P<binary>[^)]+)\)\s*$")
_FAILED = re.compile(r"^test (?P<name>\S+) \.\.\. FAILED$")


@dataclass(frozen=True)
class Failure:
    name: str
    binary: str
    crate: str  # the crate's folder, relative to the repository


def _crate_dirs(root: Path) -> dict[str, str]:
    """Crate name with `_` (as test binaries are named) -> its folder."""
    found = {}
    for manifest in sorted(root.glob("crates/*/Cargo.toml")):
        text = manifest.read_text(encoding="utf-8")
        match = re.search(r'^name\s*=\s*"([^"]+)"', text, re.M)
        if match:
            found[match.group(1).replace("-", "_")] = manifest.parent.relative_to(root).as_posix()
    return found


def parse_failures(log: str, crates: dict[str, str]) -> list[Failure]:
    """Failed tests with the binary that ran them and that binary's crate.

    cargo runs one crate's test binaries together, starting with its library's unit tests, so
    the crate of an integration test is the one whose `unittests src/lib.rs` line came last.
    """
    failures = []
    binary = crate = None
    for raw in log.splitlines():
        line = _ANSI.sub("", raw).rstrip("\r")
        running = _RUNNING.match(line)
        if running:
            binary = running.group("binary").replace("\\", "/")
            if running.group("what").startswith("unittests"):
                stem = Path(binary).name.rsplit("-", 1)[0]
                crate = crates.get(stem, crate)
            continue
        failed = _FAILED.match(line.strip())
        if failed and binary and crate:
            failures.append(Failure(failed.group("name"), binary, crate))
    return failures


def rerun(failure: Failure, root: Path) -> bool:
    crate = root / failure.crate
    binary = Path(failure.binary)
    if not binary.is_absolute():
        binary = root / binary
    environment = {**os.environ, "CARGO_MANIFEST_DIR": str(crate)}
    try:
        result = subprocess.run(
            [str(binary), failure.name, "--exact"],
            cwd=crate, env=environment, timeout=RERUN_TIMEOUT_S, check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        print(f"could not re-run {failure.name}: {error}")
        return False
    return result.returncode == 0


def report(flaky: list[Failure], failed_again: list[Failure]) -> list[str]:
    lines = []
    if flaky:
        lines += ["### FLAKY: passed only on a re-run", ""]
        lines += [f"- `{item.name}` ({item.crate})" for item in flaky]
        lines += ["", "These passed the second time. Fix them: a flaky test blocks merges and hides bugs.", ""]
    if failed_again:
        lines += ["### Failed again on a re-run", ""]
        lines += [f"- `{item.name}` ({item.crate})" for item in failed_again]
    return lines


def main(argv: list[str]) -> int:
    log = Path(argv[1]).read_text(encoding="utf-8", errors="replace")
    failures = parse_failures(log, _crate_dirs(ROOT))
    if not failures:
        print("No failed test found in the log (a build error?): not re-running.")
        return 1
    if len(failures) > MAX_RERUNS:
        print(f"{len(failures)} tests failed (more than {MAX_RERUNS}): a breakage, not re-running.")
        return 1
    flaky, failed_again = [], []
    for failure in failures:
        print(f"Re-running {failure.name} ({failure.crate}) once")
        (flaky if rerun(failure, ROOT) else failed_again).append(failure)
    for item in flaky:
        print(f"::warning title=Flaky test::{item.name} failed, then passed on a re-run ({item.crate})")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as handle:
            handle.write("\n".join(report(flaky, failed_again)) + "\n")
    print("\n".join(report(flaky, failed_again)))
    return 1 if failed_again else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
