"""python -m sheet eval FILE.csv [--raw] | get FILE.csv REF | set FILE.csv REF TEXT"""

import os
import sys

from . import Sheet, SheetError, parse_ref


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    command = argv[0] if argv else ""
    shapes = {"eval": (2, 3), "get": (3,), "set": (4,)}
    if command not in shapes or len(argv) not in shapes[command] or (len(argv) == 3 and command == "eval" and argv[2] != "--raw"):
        print("usage: python -m sheet eval FILE.csv [--raw] | get FILE.csv REF | set FILE.csv REF TEXT", file=sys.stderr)
        return 2
    path = argv[1]
    if command == "set":
        try:
            parse_ref(argv[2])
        except SheetError:
            print(f"bad reference: {argv[2]}", file=sys.stderr)
            return 2
    try:
        if command == "set" and not os.path.exists(path):
            text = ""
        else:
            with open(path, encoding="utf-8", newline="") as handle:
                text = handle.read()
    except OSError as error:
        print(f"cannot read {path}: {error.strerror}", file=sys.stderr)
        return 1
    sheet = Sheet.from_csv(text)
    if command == "eval":
        sys.stdout.write(sheet.to_csv(values=len(argv) == 2))
    elif command == "get":
        print(sheet.display(argv[2]))
    else:
        sheet.set(argv[2], argv[3])
        with open(path, "w", encoding="utf-8", newline="") as handle:
            handle.write(sheet.to_csv(values=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
