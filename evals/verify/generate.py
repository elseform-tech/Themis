"""Writes a Ledgerly project for one scenario and seed.

`ledgerly/` here is a correct, working codebase. A scenario's fault lives in it as marked
alternatives, in any comment syntax:

    # >>> plant:NAME        the planted (faulty or missing) text
    # === plant:NAME
    ...                     the correct text
    # <<< plant:NAME

and a file whose first line is `# plant-file:NAME only` exists only in NAME's planted project,
`# plant-file:NAME absent` only outside it. A project for scenario S takes the planted side of
S's blocks and the correct side of every other, so the project with no scenario is S's
reference fix. The seed varies the sample data, docs filler and log volume, never the code.
"""

import json
import os
import random
import re
import shutil
import subprocess

HERE = os.path.dirname(os.path.abspath(__file__))
TEMPLATE = os.path.join(HERE, "ledgerly")
MARK = re.compile(r"(>>>|===|<<<) plant:([a-z0-9-]+)")
FILE_MARK = re.compile(r"^\S*\s*plant-file:([a-z0-9-]+) (only|absent)\s*$")
SKIP = {"__pycache__", ".pytest_cache"}


def render(text, scenario):
    """`text` with each plant block resolved for `scenario` (None: every block correct)."""
    out, block, side = [], None, None
    for line in text.splitlines(keepends=True):
        mark = MARK.search(line)
        if mark:
            kind, name = mark.groups()
            if kind == ">>>":
                block, side = name, "planted"
            elif kind == "===" and name == block:
                side = "correct"
            elif kind == "<<<" and name == block:
                block, side = None, None
            continue
        if block is None or (side == "planted") == (block == scenario):
            out.append(line)
    return "".join(out)


def file_wanted(first_line, scenario):
    """(wanted, strip_first_line) from a file's first line."""
    mark = FILE_MARK.match(first_line.strip())
    if not mark:
        return True, False
    name, when = mark.groups()
    return (name == scenario) == (when == "only"), True


def write_code(project, scenario):
    for base, dirs, files in os.walk(TEMPLATE):
        dirs[:] = sorted(d for d in dirs if d not in SKIP)
        for name in sorted(files):
            source = os.path.join(base, name)
            target = os.path.join(project, os.path.relpath(source, TEMPLATE))
            with open(source, encoding="utf-8") as handle:
                text = handle.read()
            first, _, rest = text.partition("\n")
            wanted, strip = file_wanted(first, scenario)
            if not wanted:
                continue
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with open(target, "w", encoding="utf-8") as handle:
                handle.write(render(rest if strip else text, scenario))


def project(directory, scenario=None, seed=1, git=True):
    """Writes the project for `scenario` and `seed` into `directory` (created; must be empty)."""
    from . import filler

    os.makedirs(directory, exist_ok=True)
    write_code(directory, scenario)
    filler.write(directory, random.Random(f"ledgerly:{seed}"), scenario)
    if git:
        commit_history(directory, seed)
    return directory


def commit_history(directory, seed):
    """A git repository with a short history, like a real project."""
    env = dict(os.environ, GIT_AUTHOR_NAME="Ledgerly Dev", GIT_AUTHOR_EMAIL="dev@ledgerly.invalid",
               GIT_COMMITTER_NAME="Ledgerly Dev", GIT_COMMITTER_EMAIL="dev@ledgerly.invalid",
               GIT_AUTHOR_DATE="2026-02-02T10:00:00Z", GIT_COMMITTER_DATE="2026-02-02T10:00:00Z")

    def git(*args):
        subprocess.run(["git", *args], cwd=directory, env=env, check=True, capture_output=True)

    git("init", "-q", "-b", "main")
    with open(os.path.join(directory, ".gitignore"), "w", encoding="utf-8") as handle:
        handle.write("__pycache__/\n.pytest_cache/\n*.tmp\nledgerly.json\n")
    git("add", "ledgerly", "tests", "pytest.ini", ".gitignore")
    git("commit", "-q", "-m", "Ledgerly 0.8: invoices, stock and the CLI")
    git("add", "-A")
    git("commit", "-q", "-m", f"0.9: API, web page, docs and sample data (build {seed})")


def main(argv=None):
    import argparse

    parser = argparse.ArgumentParser(description="write one Ledgerly project to look at")
    parser.add_argument("directory")
    parser.add_argument("--scenario")
    parser.add_argument("--seed", type=int, default=1)
    args = parser.parse_args(argv)
    if os.path.exists(args.directory) and os.listdir(args.directory):
        parser.error(f"{args.directory} is not empty")
    project(args.directory, args.scenario, args.seed)
    print(json.dumps({"directory": args.directory, "scenario": args.scenario, "seed": args.seed}))


def clear(directory):
    shutil.rmtree(directory, ignore_errors=True)


if __name__ == "__main__":
    main()
