"""python -m sheet eval FILE.csv | python -m sheet get FILE.csv REF"""

import sys

from . import Sheet


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if len(argv) < 2 or argv[0] not in ("eval", "get") or (argv[0] == "get" and len(argv) != 3):
        print("usage: python -m sheet eval FILE.csv | get FILE.csv REF", file=sys.stderr)
        return 2
    try:
        with open(argv[1], encoding="utf-8", newline="") as handle:
            sheet = Sheet.from_csv(handle.read())
    except OSError as error:
        print(f"cannot read {argv[1]}: {error.strerror}", file=sys.stderr)
        return 1
    if argv[0] == "eval":
        sys.stdout.write(sheet.to_csv())
    else:
        print(sheet.display(argv[2]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
