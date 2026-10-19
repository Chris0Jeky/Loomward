"""Loopback browser fixtures with room for concurrent and overlapping document requests."""
from http.server import ThreadingHTTPServer as _ThreadingHTTPServer


class ThreadingHTTPServer(_ThreadingHTTPServer):
    # The stdlib backlog of five can refuse Chromium's sixth connection on a busy Windows host.
    request_queue_size = 128
