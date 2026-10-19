"""Mutation hardening for scripts/test_app.py (issue-free improvement; test_app.py untouched).

Loads scripts/test_app.py with its browser-only imports (playwright and the
sibling lane modules) stubbed, then pins the pure request/response logic with
fast hermetic tests: no network, no sleeps, no wall clock, no randomness, and
no files outside TemporaryDirectory. Each test names its mutant in a comment.
"""
import contextlib
import importlib.util
import io
import json
import sys
import tempfile
import types
import unittest
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

_STUB_NAMES = ('playwright', 'playwright.sync_api', 'test_app_collections',
               'test_app_a11y', 'test_app_views', 'test_app_inventory', 'test_app_copy', 'test_app_ask', 'test_app_clutter')
_ISOLATED_NAMES = (*_STUB_NAMES, 'test_app_server')


def _load_test_app():
    saved = {name: sys.modules[name] for name in _ISOLATED_NAMES if name in sys.modules}

    def stub(name, **attrs):
        mod = types.ModuleType(name)
        for key, value in attrs.items():
            setattr(mod, key, value)
        sys.modules[name] = mod
        return mod

    try:
        parent = stub('playwright')
        parent.sync_api = stub('playwright.sync_api', Page=type('Page', (), {}),
                               expect=lambda *a, **k: None,
                               sync_playwright=lambda *a, **k: None)
        stub('test_app_collections', run_collections=lambda *a, **k: None,
             run_live_collections=lambda *a, **k: None)
        stub('test_app_a11y', run_a11y=lambda *a, **k: None)
        stub('test_app_views', run_views=lambda *a, **k: None,
             wait_until=lambda *a, **k: None)
        stub('test_app_inventory', run_inventory=lambda *a, **k: None)
        stub('test_app_copy', run_copy=lambda *a, **k: None)
        stub('test_app_ask', run_ask=lambda *a, **k: None, live_ask_isolated=lambda *a, **k: None)
        stub('test_app_clutter', run_clutter=lambda *a, **k: None, live_clutter=lambda *a, **k: None)
        # The server helper has only stdlib dependencies. Load the real class even when
        # unittest discovery does not put scripts/ on sys.path (#696).
        server_spec = importlib.util.spec_from_file_location(
            'test_app_server', ROOT / 'scripts' / 'test_app_server.py')
        server_module = importlib.util.module_from_spec(server_spec)
        sys.modules['test_app_server'] = server_module
        server_spec.loader.exec_module(server_module)
        spec = importlib.util.spec_from_file_location(
            'test_app_under_test', ROOT / 'scripts' / 'test_app.py')
        module = importlib.util.module_from_spec(spec)
        sys.modules['test_app_under_test'] = module
        spec.loader.exec_module(module)
        return module
    finally:
        for name in _ISOLATED_NAMES:
            sys.modules.pop(name, None)
        sys.modules.update(saved)


test_app = _load_test_app()


class TestAppMutationPins(unittest.TestCase):
    def setUp(self):
        self._engine = test_app.engine
        self._dist = test_app.DIST
        self._wait = test_app.wait_until
        test_app.engine = test_app.Engine()

    def tearDown(self):
        test_app.engine = self._engine
        test_app.DIST = self._dist
        test_app.wait_until = self._wait

    def _handler(self, path='/', headers=None):
        handler = test_app.Handler.__new__(test_app.Handler)
        handler.path = path
        handler.headers = dict(headers or {})
        return handler

    def _capture(self, handler):
        calls = []
        handler._send = lambda status, body, ctype, extra=None: calls.append(
            (status, body, ctype, dict(extra or {})))
        return calls

    def _post(self, command, token=None, path='/api/v3/call'):
        body = json.dumps({'protocol': 'loomward/3', 'request_id': 'r1',
                           'command': command, 'payload': {}}).encode()
        handler = self._handler(path, {'Content-Type': 'application/json',
                                       'Content-Length': str(len(body)),
                                       'X-Loomward-Token': token if token is not None else test_app.TOKEN})
        handler.rfile = io.BytesIO(body)
        calls = self._capture(handler)
        with contextlib.redirect_stdout(io.StringIO()):
            handler.do_POST()
        return calls

    def _dist_with(self, tmp, files):
        dist = Path(tmp) / 'dist'
        dist.mkdir()
        for name, data in files.items():
            (dist / name).write_bytes(data)
        test_app.DIST = dist
        return dist

    def test_envelope_ok_true_when_result_given(self):
        # Mutant: `if error:` -> `if not error:` in envelope().
        out = test_app.envelope({'request_id': 'r1', 'command': 'roots.list'}, {'roots': []})
        self.assertTrue(out['ok'])
        self.assertEqual(out['result'], {'roots': []})
        self.assertEqual(out['protocol'], 'loomward/3')
        self.assertEqual(out['request_id'], 'r1')
        self.assertIn('meta', out)

    def test_envelope_error_when_error_named(self):
        # Mutant: error branch returns ok True instead of ok False.
        out = test_app.envelope({'request_id': 'r2', 'command': 'files.delete'},
                                error='capability_unavailable')
        self.assertFalse(out['ok'])
        self.assertEqual(out['error']['code'], 'capability_unavailable')
        self.assertNotIn('result', out)

    def test_fake_result_session_hello_lists_no_effects(self):
        # Mutant: session.hello effects False -> True.
        test_app.engine.dataset = 'personal'
        effects = test_app.fake_result('session.hello')['capabilities']['effects']
        self.assertEqual(len(effects), 11)
        self.assertTrue(all(value is False for value in effects.values()))

    def test_fake_result_empty_lists_and_none_for_unknown(self):
        # Mutant: fake_result returns {} instead of None for unknown commands.
        self.assertEqual(test_app.fake_result('roots.list'), {'roots': []})
        self.assertEqual(test_app.fake_result('grants.list'), {'grants': []})
        self.assertIsNone(test_app.fake_result('files.delete'))

    def test_do_post_known_command_ok_unknown_unavailable(self):
        # Mutant: ok/error dispatch branches swapped in do_POST.
        status, body, ctype, _ = self._post('roots.list')[0]
        out = json.loads(body)
        self.assertEqual((status, ctype), (200, 'application/json'))
        self.assertTrue(out['ok'])
        self.assertEqual(out['result'], {'roots': []})
        self.assertEqual(test_app.engine.calls, ['roots.list'])
        out = json.loads(self._post('files.delete')[0][1])
        self.assertFalse(out['ok'])
        self.assertEqual(out['error']['code'], 'capability_unavailable')

    def test_authorised_rejects_wrong_token(self):
        # Mutant: `token != TOKEN` -> `token == TOKEN` in _authorised.
        handler = self._handler(headers={'X-Loomward-Token': 'wrong'})
        calls = self._capture(handler)
        self.assertFalse(handler._authorised())
        self.assertEqual(calls[0][:3], (403, b'{}', 'application/json'))
        self.assertNotIn('wrong', test_app.engine.tokens_seen)

    def test_authorised_accepts_and_records_token(self):
        # Mutant: drop `engine.tokens_seen.add(token)` in _authorised.
        handler = self._handler(headers={'X-Loomward-Token': test_app.TOKEN})
        self._capture(handler)
        self.assertTrue(handler._authorised())
        self.assertIn(test_app.TOKEN, test_app.engine.tokens_seen)

    def test_do_post_engine_down_is_503(self):
        # Mutant: drop the `engine.down` guard in do_POST.
        test_app.engine.down = True
        calls = self._post('roots.list')
        self.assertEqual(calls[0][0], 503)
        self.assertEqual(test_app.engine.calls, [])

    def test_do_get_events_engine_down_is_503(self):
        # Mutant: drop the `engine.down` guard in do_GET /api/v3/events.
        test_app.engine.down = True
        handler = self._handler('/api/v3/events', {'X-Loomward-Token': test_app.TOKEN})
        calls = self._capture(handler)
        handler.do_GET()
        self.assertEqual(calls[0][0], 503)

    def test_do_get_events_records_last_event_id(self):
        # Mutant: drop the Last-Event-ID recording in do_GET /api/v3/events.
        test_app.engine.down = True  # returns before the heartbeat loop: no sleep.
        handler = self._handler('/api/v3/events', {'X-Loomward-Token': test_app.TOKEN,
                                                   'Last-Event-ID': f'{test_app.EPOCH}.0'})
        handler._send = lambda *a, **k: None
        handler.do_GET()
        self.assertIn(f'{test_app.EPOCH}.0', test_app.engine.resume_ids)

    def test_do_get_static_traversal_is_404(self):
        # Mutant: traversal guard `not in` -> `in` (or the guard dropped).
        with tempfile.TemporaryDirectory() as tmp:
            self._dist_with(tmp, {'index.html': b'<html></html>'})
            (Path(tmp) / 'secret.txt').write_bytes(b'secret')
            handler = self._handler('/../secret.txt')
            calls = self._capture(handler)
            handler.do_GET()
        self.assertEqual(calls[0][0], 404)

    def test_do_get_empty_path_serves_index(self):
        # Mutant: drop `''` from the `('/', '')` index mapping.
        with tempfile.TemporaryDirectory() as tmp:
            self._dist_with(tmp, {'index.html': b'<html>index</html>'})
            handler = self._handler('')
            calls = self._capture(handler)
            handler.do_GET()
        self.assertEqual(calls[0][:3], (200, b'<html>index</html>', 'text/html; charset=utf-8'))

    def test_do_get_missing_file_is_404(self):
        # Mutant: drop the missing-file -> 404 error branch in do_GET.
        with tempfile.TemporaryDirectory() as tmp:
            self._dist_with(tmp, {'index.html': b'<html></html>'})
            handler = self._handler('/nope-missing.txt')
            calls = self._capture(handler)
            handler.do_GET()
        self.assertEqual(calls[0][:3], (404, b'not found', 'text/plain'))

    def test_check_raises_on_false_and_prints_on_true(self):
        # Mutant: `if not cond:` -> `if cond:` in check().
        with self.assertRaises(AssertionError):
            test_app.check(False, 'm')
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            test_app.check(True, 'm')
        self.assertIn('PASS m', buf.getvalue())

    def test_token_is_32_hex_chars(self):
        # Mutant: TOKEN shortened by one char.
        self.assertEqual(len(test_app.TOKEN), 32)
        self.assertRegex(test_app.TOKEN, r'\A[0-9a-f]{32}\Z')

    def test_personal_fixture_totals_and_duplicate_pair(self):
        # Mutant: a PERSONAL_FIXTURE size moved by one.
        fixture = test_app.PERSONAL_FIXTURE
        self.assertEqual(len(fixture), 5)
        self.assertEqual(sum(size for _, size in fixture), 145)
        sizes = Counter(size for _, size in fixture)
        self.assertEqual(sizes[10], 2)
        self.assertTrue(all(count == 1 for size, count in sizes.items() if size != 10))

    def test_do_post_records_command_before_answering(self):
        # Mutant: return early before `engine.calls.append` in do_POST.
        self._post('roots.list')
        self.assertEqual(test_app.engine.calls, ['roots.list'])

    def test_check_resume_waits_then_accepts_epoch_ids(self):
        # Mutant: drop the wait_until call in check_resume (early return).
        seen = []
        test_app.wait_until = lambda page, expr, arg=None, timeout_ms=9000: seen.append(expr)
        test_app.engine.resume_ids.append(f'{test_app.EPOCH}.0')
        with contextlib.redirect_stdout(io.StringIO()):
            test_app.check_resume(object())
        self.assertEqual(len(seen), 1)

    def test_check_resume_rejects_bare_or_foreign_ids(self):
        # Mutant: `all(...)` -> `any(...)` in check_resume; killed before by test_app_timing.
        test_app.wait_until = lambda *a, **k: None
        test_app.engine.resume_ids.extend([f'{test_app.EPOCH}.0', '0'])
        with self.assertRaises(AssertionError):
            test_app.check_resume(object())

    def test_check_resume_rejects_other_epochs(self):
        # Mutant: epoch-qualified fullmatch -> non-empty check; killed before by test_app_timing.
        test_app.wait_until = lambda *a, **k: None
        test_app.engine.resume_ids.append('another_epoch.0')
        with self.assertRaises(AssertionError):
            test_app.check_resume(object())


if __name__ == '__main__':
    unittest.main()
