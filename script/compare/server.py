#!/usr/bin/env python3
"""Blind A/B voting on stacker renders.

    python3 script/compare/server.py [data_dir] [--port 8765] [--host 127.0.0.1]

`data_dir` (default `target/compare`) holds one folder per dataset with
`<name>.png` renders and `pairs.json`: a list of {"dataset": ..., "a": name, "b": name}. Each pair
is shown with its sides shuffled; votes are appended to `votes.jsonl` with the
real names. Open http://127.0.0.1:<port>. `--host 0.0.0.0` serves it to other
machines too (a tailnet, say).
"""

import json
import random
import sys
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote

HERE = Path(__file__).resolve().parent
def option(name, default):
    return sys.argv[sys.argv.index(name) + 1] if name in sys.argv else default


HOST = option("--host", "127.0.0.1")
PORT = int(option("--port", "8765"))
flag_values = {option("--host", None), option("--port", None)}
args = [a for a in sys.argv[1:] if not a.startswith("--") and a not in flag_values]
DATA = Path(args[0] if args else "target/compare").resolve()


def pairs():
    out = []
    for i, p in enumerate(json.loads((DATA / "pairs.json").read_text())):
        flip = random.random() < 0.5
        left, right = (p["b"], p["a"]) if flip else (p["a"], p["b"])
        out.append({"id": i, "dataset": p["dataset"], "left": left, "right": right})
    return out


class Handler(SimpleHTTPRequestHandler):
    def translate_path(self, path):
        path = unquote(path.split("?", 1)[0].split("#", 1)[0])
        if path in ("/", "/index.html"):
            return str(HERE / "index.html")
        # Resolved, so `..` can't climb out of DATA; "" opens as a 404.
        target = (DATA / path.lstrip("/")).resolve()
        return str(target) if target.is_relative_to(DATA) else ""

    def do_GET(self):
        if self.path.startswith("/api/pairs"):
            # Names go to the page so it can load the files; the page never
            # shows them until after the vote.
            return self._json(pairs())
        return super().do_GET()

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        with open(DATA / "votes.jsonl", "a") as f:
            f.write(json.dumps(body) + "\n")
        self._json({"ok": True})

    def _json(self, obj):
        data = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *a):
        pass


if __name__ == "__main__":
    print(f"serving {DATA} on http://{HOST}:{PORT}")
    ThreadingHTTPServer((HOST, PORT), Handler).serve_forever()
