"""Routes for the JSON API, and a small development server that also serves web/.

    python3 -m ledgerly.api.routes --store ledgerly.json --port 8000
"""

import argparse
import json
import os
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

from ledgerly.api import handlers
from ledgerly.storage.json_store import Store

WEB = os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))), "web")


def handle(store, method, path, body=None):
    """(status, payload) for one API request."""
    url = urlparse(path)
    query = {key: values[-1] for key, values in parse_qs(url.query).items()}
    parts = [part for part in url.path.split("/") if part]
    try:
        if method == "GET" and parts == ["api", "invoices"]:
            return 200, handlers.list_invoices(store, query)
        if method == "POST" and parts == ["api", "invoices"]:
            return 201, handlers.create_invoice(store, body or {})
        if method == "POST" and len(parts) == 4 and parts[:2] == ["api", "invoices"] and parts[3] == "payments":
            return 200, handlers.pay(store, parts[2], body or {})
    except KeyError as error:
        return 404, {"error": str(error.args[0] if error.args else error)}
    except ValueError as error:
        return 400, {"error": str(error)}
    return 404, {"error": f"no route for {method} {url.path}"}


def serve(store, port):
    class Handler(SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=WEB, **kwargs)

        def _api(self, method):
            length = int(self.headers.get("Content-Length") or 0)
            body = json.loads(self.rfile.read(length) or b"{}") if length else None
            status, payload = handle(store, method, self.path, body)
            data = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            if self.path.startswith("/api/"):
                self._api("GET")
            else:
                super().do_GET()

        def do_POST(self):
            self._api("POST")

    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--store", default="ledgerly.json")
    parser.add_argument("--port", type=int, default=8000)
    args = parser.parse_args()
    serve(Store.load(args.store), args.port)
