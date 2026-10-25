"""Checks the structure of out/ and the index total (python3 check.py). Exit 0 when it is right.

It does not check counts, statuses, owners or summaries: those come from reading the memos.
"""

import hashlib
import os
import re
import sys

ROOT = os.path.dirname(os.path.abspath(__file__))
STATUSES = ("active", "paused", "retired")
TOTAL_DIGEST = "{{total_digest}}"
SALT = "{{salt}}"


def field(text, name):
    match = re.search(rf"^\s*-\s*{name}:\s*(.+?)\s*$", text, re.MULTILINE)
    return match.group(1) if match else None


def problems():
    names = sorted(name[:-4] for name in os.listdir(os.path.join(ROOT, "src")) if name.endswith(".txt"))
    found = []
    out = os.path.join(ROOT, "out")
    if not os.path.isdir(out):
        return ["missing out/"]
    for file in sorted(os.listdir(out)):
        if file == "INDEX.md" or not file.endswith(".md"):
            continue
        name = file[:-3]
        if name not in names:
            found.append(f"out/{file}: no source src/{name}.txt")
            continue
        with open(os.path.join(out, file), encoding="utf-8") as handle:
            text = handle.read()
        if not re.search(rf"^#\s*{re.escape(name)}\s*$", text, re.MULTILINE):
            found.append(f"out/{file}: no '# {name}' line")
        count = field(text, "count")
        if count is None or not re.fullmatch(r"\d+", count):
            found.append(f"out/{file}: '- count:' must be a number in digits")
        status = field(text, "status")
        if status not in STATUSES:
            found.append(f"out/{file}: '- status:' must be one of {', '.join(STATUSES)}")
        summary = field(text, "summary")
        if summary is None or len(summary.split()) < 3:
            found.append(f"out/{file}: '- summary:' must be a sentence")
        since = field(text, "since")
        if since is not None and not re.fullmatch(r"(19|20)\d\d", since):
            found.append(f"out/{file}: '- since:' must be a year")
    index = os.path.join(out, "INDEX.md")
    if not os.path.isfile(index):
        found.append("missing out/INDEX.md")
        return found
    with open(index, encoding="utf-8") as handle:
        text = handle.read()
    rows = re.findall(r"^\s*-\s*([a-z]+-[a-z]+):\s*(\S+)\s*$", text, re.MULTILINE)
    listed = [name for name, _status in rows]
    for name in names:
        times = listed.count(name)
        if times != 1:
            found.append(f"out/INDEX.md: {name} is listed {times} times")
    for name, status in rows:
        if status not in STATUSES:
            found.append(f"out/INDEX.md: {name} has status {status}, not one of {', '.join(STATUSES)}")
    if listed != sorted(listed):
        found.append("out/INDEX.md: items are not sorted by name")
    total = re.search(r"^\s*total:\s*(\d+)\s*$", text, re.MULTILINE)
    if not total:
        found.append("out/INDEX.md: no 'total: <number>' line")
    elif hashlib.sha256(f"{SALT}:{int(total.group(1))}".encode()).hexdigest() != TOTAL_DIGEST:
        found.append("out/INDEX.md: the total is not the sum of every item's count (retired items included); "
                     "recheck the counts")
    return found


if __name__ == "__main__":
    issues = problems()
    if issues:
        print(f"{len(issues)} problem(s):")
        for issue in issues[:15]:
            print(f"- {issue}")
        if len(issues) > 15:
            print(f"- ... and {len(issues) - 15} more")
        sys.exit(1)
    print("OK: structure and total are right")
