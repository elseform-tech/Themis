"""Prints the image type of every file in a folder (used by the thumbnail job)."""

import imghdr
import sys
from pathlib import Path


def kind(path):
    """'png', 'jpeg', 'gif' or None."""
    return imghdr.what(path)


def main(folder):
    for path in sorted(Path(folder).iterdir()):
        print(path.name, kind(path) or "unknown")


if __name__ == "__main__":
    main(sys.argv[1])
