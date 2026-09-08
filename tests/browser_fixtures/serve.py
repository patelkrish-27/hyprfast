#!/usr/bin/env python3
"""Minimal local HTTP harness for BrowserRuntime fixture tests (Phase 0B).

Serves tests/browser_fixtures/ over HTTP on localhost so later phases have
a real, controllable network target. file:// URLs alone cannot exercise
Page.lifecycleEvent timing the way a deliberately-delayed local HTTP
response can (see plan Phase 0B / Phase 4 DoD).

Usage:
    python3 tests/browser_fixtures/serve.py [--port 8901] [--dir tests/browser_fixtures]

Endpoints:
    /<fixture>.html        served statically from the fixtures directory.
    /slow?target=<file>&delay=<seconds>
                           sleeps <delay> seconds (default 2.0), then serves
                           <file> (default navigation.html). Used by Phase 4
                           delayed-navigation timing tests to prove completion
                           detection is event-driven, not a fixed sleep.
    /healthz               returns "ok" (for wait-for-ready polling).

Only stdlib is used. Binds 127.0.0.1 only. Quiet logging (one line per
request to stderr).
"""

import argparse
import os
import time
import urllib.parse
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer


class FixtureHandler(SimpleHTTPRequestHandler):
    server_version = "FixtureHarness/0B"

    def log_message(self, fmt, *args):
        # Single quiet log line per request.
        sys_stderr_write(f"{self.address_string()} {self.command} {self.path}\n")

    def do_GET(self):
        parsed = urllib.parse.urlparse(self.path)
        if parsed.path == "/healthz":
            body = b"ok\n"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if parsed.path == "/slow":
            qs = urllib.parse.parse_qs(parsed.query)
            target = qs.get("target", ["navigation.html"])[0]
            try:
                delay = float(qs.get("delay", ["2.0"])[0])
            except ValueError:
                delay = 2.0
            # Clamp to a sane range so a typo can't hang the harness.
            delay = max(0.0, min(delay, 30.0))
            # Safety: only serve files inside the fixtures directory.
            target = os.path.basename(target)
            time.sleep(delay)
            self.path = "/" + target
            return super().do_GET()
        return super().do_GET()

    # Silence the default stderr write path duplication; use sys directly.
    def log_error(self, fmt, *args):
        sys_stderr_write(f"error: {fmt % args}\n")


def sys_stderr_write(msg):
    import sys

    sys.stderr.write(msg)
    sys.stderr.flush()


def main():
    ap = argparse.ArgumentParser(description="Serve BrowserRuntime HTML fixtures.")
    ap.add_argument("--port", type=int, default=8901)
    ap.add_argument(
        "--dir",
        default=os.path.join(os.path.dirname(os.path.abspath(__file__))),
    )
    args = ap.parse_args()
    os.chdir(args.dir)
    server = ThreadingHTTPServer(("127.0.0.1", args.port), FixtureHandler)
    print(f"serving {args.dir} at http://127.0.0.1:{args.port}/")
    print("endpoints: /<fixture>.html  /slow?target=navigation.html&delay=2.0  /healthz")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
