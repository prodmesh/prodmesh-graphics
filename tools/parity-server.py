#!/usr/bin/env python3
"""Serve the parity harness and accept raw RGBA POSTs back from the browser.

Deliberately tiny and dependency-free. It exists so the browser can hand back a
megabyte of raw pixels without going through a canvas encode or a base64 round
trip through the devtools console — see web/parity.html for why raw bytes are
the only honest path here.
"""
import os, sys
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

ROOT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "web")
OUT = os.path.abspath(sys.argv[2] if len(sys.argv) > 2 else "testdata/golden/wasm")
os.makedirs(OUT, exist_ok=True)


class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *a, **kw):
        super().__init__(*a, directory=ROOT, **kw)

    def do_POST(self):
        if not self.path.startswith("/put/"):
            self.send_error(404)
            return
        name = os.path.basename(self.path[len("/put/"):])
        n = int(self.headers.get("Content-Length", 0))
        data = self.rfile.read(n)
        with open(os.path.join(OUT, name + ".rgba"), "wb") as f:
            f.write(data)
        print(f"  received {name}: {len(data)} bytes", flush=True)
        self.send_response(204)
        self.end_headers()

    def log_message(self, *a):
        pass


if __name__ == "__main__":
    port = int(os.environ.get("PORT", "8532"))
    print(f"serving {ROOT} on :{port}, writing to {OUT}", flush=True)
    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
