#!/usr/bin/env python3
"""Serve the repository's tests/ directory on the loopback interface, for the media checks.

    python3 scripts/serve-tests.py [--port N]

Binds 127.0.0.1 on an ephemeral port (or --port), prints the port on the first line of
standard output, and serves tests/ (so tests/pages/media.html is /pages/media.html). Single
byte ranges ("Range: bytes=a-b", "a-" and "-n") are answered with 206 Partial Content, as real
media servers do; python's own http.server ignores Range. /private asks for HTTP Basic
authentication (realm "Mercury test realm"; it accepts only the fixed test credentials
"alice" / "secret"), for tests/pages/http-auth.html. Exits on SIGINT, SIGTERM or SIGHUP, and
when the process that started it goes away. Standard library only.
"""

import argparse
import base64
import os
import re
import signal
import sys
import threading
import time
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

RANGE = re.compile(r"^bytes=(\d*)-(\d*)$")
PRIVATE_PATH = "/private"
PRIVATE_CREDENTIALS = "Basic " + base64.b64encode(b"alice:secret").decode("ascii")


class Handler(SimpleHTTPRequestHandler):
    def log_message(self, format, *args):  # quiet: the callers print their own results
        pass

    def end_headers(self):
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def do_GET(self):
        if self.path.split("?", 1)[0] == PRIVATE_PATH:
            self.send_private(include_body=True)
            return
        super().do_GET()

    def do_HEAD(self):
        if self.path.split("?", 1)[0] == PRIVATE_PATH:
            self.send_private(include_body=False)
            return
        super().do_HEAD()

    def send_private(self, include_body):
        """401 with a Basic challenge unless the test credentials are sent."""
        if self.headers.get("Authorization") == PRIVATE_CREDENTIALS:
            status, body = 200, b"Signed in.\n"
        else:
            status, body = 401, b"Sign-in required.\n"
        self.send_response(status)
        if status == 401:
            self.send_header("WWW-Authenticate", 'Basic realm="Mercury test realm"')
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if include_body:
            self.wfile.write(body)

    def send_head(self):
        self._remaining = None
        header = self.headers.get("Range")
        path = self.translate_path(self.path)
        if header is None or os.path.isdir(path) or not os.path.isfile(path):
            return super().send_head()
        match = RANGE.match(header.strip())
        if match is None or (match.group(1) == "" and match.group(2) == ""):
            return super().send_head()  # several ranges or another unit: send it whole
        size = os.path.getsize(path)
        first, last = match.group(1), match.group(2)
        if first == "":  # the last n bytes
            start, end = max(0, size - int(last)), size - 1
        else:
            start = int(first)
            end = min(int(last), size - 1) if last else size - 1
        if size == 0 or start >= size or start > end:
            self.send_response(416)
            self.send_header("Content-Range", "bytes */%d" % size)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return None
        handle = open(path, "rb")
        handle.seek(start)
        self.send_response(206)
        self.send_header("Content-Type", self.guess_type(path))
        self.send_header("Content-Range", "bytes %d-%d/%d" % (start, end, size))
        self.send_header("Content-Length", str(end - start + 1))
        self.end_headers()
        self._remaining = end - start + 1
        return handle

    def copyfile(self, source, outputfile):
        if self._remaining is None:
            return super().copyfile(source, outputfile)
        remaining = self._remaining
        while remaining > 0:
            chunk = source.read(min(65536, remaining))
            if not chunk:
                break
            outputfile.write(chunk)
            remaining -= len(chunk)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--port", type=int, default=0, help="port to bind (default: any free port)")
    arguments = parser.parse_args()

    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), os.pardir, "tests")
    os.chdir(os.path.realpath(root))
    server = ThreadingHTTPServer(("127.0.0.1", arguments.port), Handler)
    server.daemon_threads = True

    def stop(*_):
        threading.Thread(target=server.shutdown, daemon=True).start()

    for signal_number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(signal_number, stop)

    parent = os.getppid()
    if parent == 1:  # the starting process has already gone: nobody will stop this server
        server.server_close()
        return 0

    def watch_parent():
        while os.getppid() == parent:
            time.sleep(0.5)
        stop()

    threading.Thread(target=watch_parent, daemon=True).start()

    print(server.server_address[1], flush=True)
    try:
        server.serve_forever(poll_interval=0.2)
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
