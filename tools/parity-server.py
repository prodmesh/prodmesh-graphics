#!/usr/bin/env python3
"""Serve the parity harness, hand it a font, and accept raw RGBA POSTs back.

Deliberately tiny and dependency-free. It exists so the browser can hand back a
megabyte of raw pixels without going through a canvas encode or a base64 round
trip through the devtools console — see web/parity.html for why raw bytes are
the only honest path here.

    tools/parity-server.py [root] [outdir] [font]

`font` — or $PMG_FONT — is served at /font, and it must be THE SAME FILE the
native side was given with `pmgctl render-golden --font`. pmg-text has no system
font database; if the two halves are handed different faces, the diff measures
two typefaces and reports it as a renderer difference.
"""
import os, sys
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

ROOT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "web")
OUT = os.path.abspath(sys.argv[2] if len(sys.argv) > 2 else "testdata/golden/wasm")
FONT = sys.argv[3] if len(sys.argv) > 3 else os.environ.get("PMG_FONT")
os.makedirs(OUT, exist_ok=True)


class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *a, **kw):
        super().__init__(*a, directory=ROOT, **kw)

    def do_GET(self):
        if urlparse(self.path).path == "/font":
            if not FONT or not os.path.exists(FONT):
                # 404 rather than an empty body: the harness must be able to
                # tell "no font here" from "a zero-byte face", and skip the text
                # figures loudly instead of rendering blanks.
                self.send_error(404, "no font configured")
                return
            data = open(FONT, "rb").read()
            self.send_response(200)
            self.send_header("Content-Type", "font/ttf")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        super().do_GET()

    def do_POST(self):
        u = urlparse(self.path)
        if not u.path.startswith("/put/"):
            self.send_error(404)
            return
        name = os.path.basename(u.path[len("/put/"):])
        q = parse_qs(u.query)
        n = int(self.headers.get("Content-Length", 0))
        data = self.rfile.read(n)
        # Raw bytes carry no dimensions, so the NAME does: <figure>.<w>x<h>.rgba.
        # Guessing a square from the byte count worked while every figure was
        # 512x512 and stopped working the moment the corpus grew 16:9 templates.
        w, h = q.get("w", [None])[0], q.get("h", [None])[0]
        suffix = f".{w}x{h}" if w and h else ""
        with open(os.path.join(OUT, f"{name}{suffix}.rgba"), "wb") as f:
            f.write(data)
        print(f"  received {name}: {len(data)} bytes", flush=True)
        self.send_response(204)
        self.end_headers()

    def log_message(self, *a):
        pass


if __name__ == "__main__":
    port = int(os.environ.get("PORT", "8532"))
    print(f"serving {ROOT} on :{port}, writing to {OUT}", flush=True)
    if FONT and os.path.exists(FONT):
        b = open(FONT, "rb").read()
        print(f"font    {FONT} ({len(b)} bytes)", flush=True)
        print("        the page prints its blake3 — compare that with what", flush=True)
        print("        `pmgctl render-golden` printed", flush=True)
    else:
        print("font    NONE — text figures will be skipped", flush=True)
    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
