"""`python3 -m ledgerly.cli.main [--store FILE] COMMAND ...` (see docs/CLI.md)."""

import argparse
import sys

from ledgerly.cli.commands import customers, import_cmd, invoices, reports, stock
from ledgerly.storage.json_store import Store

GROUPS = (customers, invoices, reports, stock, import_cmd)


def main(argv=None, out=None):
    out = out or sys.stdout
    parser = argparse.ArgumentParser(prog="ledgerly")
    parser.add_argument("--store", default="ledgerly.json", help="the JSON store (default ./ledgerly.json)")
    parser.add_argument("--rates", help="a rates.csv to use instead of the built-in VAT rates")
    groups = parser.add_subparsers(dest="group", required=True)
    for group in GROUPS:
        group.register(groups)
    args = parser.parse_args(argv)
    store = Store.load(args.store)
    try:
        return args.run(store, args, out) or 0
    except (KeyError, ValueError) as error:
        print(f"error: {error.args[0] if error.args else error}", file=out)
        return 1


if __name__ == "__main__":
    sys.exit(main())
