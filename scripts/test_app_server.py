"""Loopback browser fixtures with room for concurrent and overlapping document requests."""
from http.server import ThreadingHTTPServer as _ThreadingHTTPServer


class ThreadingHTTPServer(_ThreadingHTTPServer):
    # The stdlib backlog of five can refuse Chromium's sixth connection on a busy Windows host.
    request_queue_size = 128


CALL = '**/api/v3/call'


def route_call(page, handler, url=CALL):
    """Route `url` through `handler` until unroute_call(); one routed handler per page at a time."""
    active = getattr(page, '_lw_routed', None)
    if active is not None:
        raise AssertionError(f'{active.__name__} is still routed; unroute_call it first')
    page._lw_routed = handler
    page.route(url, handler)


def unroute_call(page, handler):
    """Remove `handler` only after its in-flight invocations finish; a no-op when it is not the routed one.

    Page.unroute() does not wait: the Playwright server falls back a request whose handler is removed
    mid-flight, so a handler still inside route.fetch() then fails with 'Route is already handled!',
    raised into whatever call runs next (main run 43362934378). unroute_all(behavior='wait') waits first,
    which is why route_call() allows one handler per page. Finish or abort held routes before calling this.
    """
    if getattr(page, '_lw_routed', None) is not handler:
        return
    page.unroute_all(behavior='wait')
    page._lw_routed = None
