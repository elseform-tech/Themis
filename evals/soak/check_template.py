"""Checks the facts in out/ against src/ (python3 check.py). Exit 0 when everything matches."""

import os
import re
import sys

ROOT = os.path.dirname(os.path.abspath(__file__))
FIELDS = ("region", "count", "status")


def facts(path):
    found = {}
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            match = re.match(r"^(name|owner|region|count|status):\s*(\S+)\s*$", line)
            if match:
                found.setdefault(match.group(1), match.group(2))
    return found


def listed(text, field):
    match = re.search(rf"^\s*-\s*{field}:\s*(\S+)\s*$", text, re.MULTILINE)
    return match.group(1) if match else None


def problems():
    items = []
    src = os.path.join(ROOT, "src")
    for name in sorted(os.listdir(src)):
        if name.endswith(".txt"):
            items.append(facts(os.path.join(src, name)))
    found = []
    for item in items:
        if item.get("status") == "retired":
            continue
        path = os.path.join(ROOT, "out", f"{item['name']}.md")
        if not os.path.isfile(path):
            found.append(f"missing out/{item['name']}.md")
            continue
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
        if not re.search(rf"^#\s*{re.escape(item['name'])}\s*$", text, re.MULTILINE):
            found.append(f"out/{item['name']}.md: no '# {item['name']}' line")
        for field in FIELDS:
            value = listed(text, field)
            if value is None:
                found.append(f"out/{item['name']}.md: no '- {field}:' line")
            elif value.lower() != item[field].lower():
                found.append(f"out/{item['name']}.md: {field} is {value}, the source says {item[field]}")
    index = os.path.join(ROOT, "out", "INDEX.md")
    if not os.path.isfile(index):
        found.append("missing out/INDEX.md")
        return found
    with open(index, encoding="utf-8") as handle:
        text = handle.read()
    rows = re.findall(r"^\s*-\s*([a-z]+-[a-z]+):\s*(\S+)\s*$", text, re.MULTILINE)
    seen = [name for name, _status in rows]
    for item in items:
        times = seen.count(item["name"])
        if times != 1:
            found.append(f"out/INDEX.md: {item['name']} is listed {times} times")
            continue
        status = dict(rows)[item["name"]]
        if status.lower() != item["status"].lower():
            found.append(f"out/INDEX.md: {item['name']} is {status}, the source says {item['status']}")
    if seen != sorted(seen):
        found.append("out/INDEX.md: items are not sorted by name")
    expected = sum(int(item["count"]) for item in items)
    total = re.search(r"^\s*total:\s*(\d+)\s*$", text, re.MULTILINE)
    if not total:
        found.append("out/INDEX.md: no 'total: <number>' line")
    elif int(total.group(1)) != expected:
        found.append(f"out/INDEX.md: total is {total.group(1)}, expected the sum of all counts")
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
    print("OK: every output matches its source")
