"""Headless Chrome against a local static server: the DOM after the page's scripts ran, and
the console errors. Used by the graders only; the pages run in Chrome's own sandbox."""

import contextlib
import functools
import http.server
import os
import re
import shutil
import subprocess
import tempfile
import threading

CHROME_CANDIDATES = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "google-chrome", "chromium", "chromium-browser",
]


def chrome():
    """Path of a usable Chrome or Chromium, or None."""
    for candidate in CHROME_CANDIDATES:
        path = candidate if os.path.isabs(candidate) else shutil.which(candidate)
        if path and os.path.exists(path):
            return path
    return None


class _QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


@contextlib.contextmanager
def serve(root):
    """A localhost static server for `root` (modules need http, not file://)."""
    handler = functools.partial(_QuietHandler, directory=root)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_address[1]}"
    finally:
        server.shutdown()
        server.server_close()


def dump_dom(url, budget_ms=15000, timeout=120):
    """(DOM after scripts ran, console error lines). WebGL runs on SwiftShader."""
    binary = chrome()
    if not binary:
        raise RuntimeError("no Chrome or Chromium found")
    profile = tempfile.mkdtemp(prefix="hard-eval-chrome-")
    try:
        done = subprocess.run(
            [binary, "--headless=new", f"--user-data-dir={profile}", "--no-first-run",
             "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist",
             "--enable-logging=stderr", "--v=0", f"--virtual-time-budget={budget_ms}",
             "--window-size=1280,720", "--dump-dom", url],
            capture_output=True, text=True, timeout=timeout, check=False)
    finally:
        shutil.rmtree(profile, ignore_errors=True)
    errors = [line for line in done.stderr.splitlines()
              if "CONSOLE" in line and re.search(r"Uncaught|Error|error", line)]
    return done.stdout, errors
