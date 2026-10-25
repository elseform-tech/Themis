"""Reference `python3 -m ledger report DATA_DIR --today YYYY-MM-DD` (shipped as ledger/__main__.py
in the reference solution only)."""

import csv
import datetime
import importlib
import os
import sys

from ledger.desks import DESKS

FIELDS = ("lines", "rejected", "fees", "released", "held")


def money(cents):
    return f"{cents // 100}.{cents % 100:02d}"


def load(path):
    if not os.path.isfile(path):
        return []
    with open(path, newline="", encoding="utf-8") as handle:
        return [{"id": row["id"], "account": row["account"], "amount": int(row["amount"]),
                 "currency": row["currency"], "date": row["date"]} for row in csv.DictReader(handle)]


def main(argv):
    if len(argv) != 4 or argv[0] != "report" or argv[2] != "--today":
        print("usage: python3 -m ledger report DATA_DIR --today YYYY-MM-DD", file=sys.stderr)
        return 2
    try:
        datetime.date.fromisoformat(argv[3])
    except ValueError:
        print(f"bad date: {argv[3]}", file=sys.stderr)
        return 2
    rows = []
    for name in DESKS:
        desk = importlib.import_module(f"ledger.desks.{name}")
        entries = load(os.path.join(argv[1], f"{name}.csv"))
        settled = desk.settle(entries)
        held = desk.holds(entries, argv[3])
        rows.append((desk.RULES["code"], {"lines": len(settled["lines"]), "rejected": len(settled["errors"]),
                                          "fees": sum(line["fee"] for line in settled["lines"]),
                                          "released": len(held["released"]), "held": len(held["held"])}))
    rows.sort()
    for code, counts in rows:
        print(code + " " + " ".join(f"{key}={money(counts[key]) if key == 'fees' else counts[key]}" for key in FIELDS))
    totals = {key: sum(counts[key] for _, counts in rows) for key in FIELDS}
    print(f"TOTAL desks={len(rows)} " + " ".join(
        f"{key}={money(totals[key]) if key == 'fees' else totals[key]}" for key in FIELDS))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
