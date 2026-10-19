"""Small regression checks for browser gate sampling and asynchronous server assertions."""
import contextlib
import importlib.util
import io
import json
import socket
import time
import unittest
from http.client import HTTPConnection
from pathlib import Path
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[1]
import test_app
import test_app_views

spec = importlib.util.spec_from_file_location('atlas_e2e', ROOT / 'app/tests/e2e/test_atlas.py')
atlas = importlib.util.module_from_spec(spec)
spec.loader.exec_module(atlas)


class AppE2ETimingTests(unittest.TestCase):
    def test_fake_servers_accept_a_browser_burst_before_dispatch(self):
        for server_type in (test_app.ThreadingHTTPServer, test_app_views.ThreadingHTTPServer):
            with self.subTest(server=server_type), contextlib.ExitStack() as cleanup:
                server = server_type(('127.0.0.1', 0), test_app.Handler)
                cleanup.callback(server.server_close)
                # Simulate a busy dispatcher: connections queue before any handler can run.
                # Chromium can open six HTTP/1 connections, plus ones from a replaced document.
                for _ in range(12):
                    connection = socket.create_connection(server.server_address, timeout=0.5)
                    cleanup.callback(connection.close)

    def test_fake_engine_broadcasts_changes_to_each_event_stream(self):
        engine = test_app_views.FakeEngine()
        engine.start()
        self.addCleanup(engine.server.server_close)
        self.addCleanup(engine.stop)

        def frame(response):
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                line = response.readline()
                if line.startswith(b'data: '):
                    return json.loads(line[6:])
                self.assertTrue(line, 'event stream ended before the event')
            self.fail('event was not broadcast to each stream')

        streams = []
        for _ in range(2):
            connection = HTTPConnection('127.0.0.1', engine.server.server_port, timeout=2)
            self.addCleanup(connection.close)
            connection.request('GET', '/api/v3/events', headers={'X-Loomward-Token': test_app_views.TOKEN})
            response = connection.getresponse()
            self.addCleanup(response.close)
            self.assertEqual(response.status, 200)
            self.assertEqual(frame(response)['event'], 'stream.hello')
            streams.append(response)

        # A previous page's connection can overlap its replacement during navigation.
        engine.land('E:\\Picked After The Deadline')
        for response in streams:
            event = frame(response)
            self.assertEqual(event['event'], 'roots.changed')
            self.assertEqual(event['data']['roots'][0]['display_path']['text'], 'E:\\Picked After The Deadline')
        engine.invalidate()
        for response in streams:
            self.assertEqual(frame(response)['event'], 'tree.invalidated')

    def test_resume_waits_for_expected_id_and_rejects_bare_ids(self):
        page = Mock()
        with patch.object(test_app, 'engine', test_app.Engine()), contextlib.redirect_stdout(io.StringIO()):
            page.wait_for_timeout.side_effect = lambda _: test_app.engine.resume_ids.append(f'{test_app.EPOCH}.0')
            test_app.check_resume(page)
            page.wait_for_timeout.assert_called_once_with(50)
            page.evaluate.assert_not_called()
            test_app.engine.resume_ids.append('0')
            with self.assertRaises(AssertionError):
                test_app.check_resume(page)
            test_app.engine.resume_ids[:] = ['another_epoch.0']
            with patch.object(test_app_views.time, 'monotonic', side_effect=[0, 10]):
                with self.assertRaisesRegex(AssertionError, 'timed out'):
                    test_app.check_resume(page)

    def test_wait_times_out_and_keeps_javascript_polling(self):
        with patch.object(test_app_views.time, 'monotonic', side_effect=[0, 10]):
            with self.assertRaisesRegex(AssertionError, 'timed out'):
                test_app_views.wait_until(None, lambda: False)
        page = Mock()
        page.evaluate.side_effect = [False, True]
        test_app_views.wait_until(page, 'predicate', 7)
        page.wait_for_timeout.assert_called_once_with(50)
        self.assertEqual(page.evaluate.call_args_list, [unittest.mock.call('predicate', 7)] * 2)

    def test_p9_uses_five_fresh_samples_after_warmup(self):
        page = Mock()
        values = iter([999, 45, 70, 40, 48, 42])

        def marks(*_):
            value = next(values)
            return [{'name': name, 'ms': ms, 'nodes': 2500, 'cells': 100}
                    for name, ms in [('layout', value - 10), ('first-paint', 10), ('slice-to-paint', value + 5)]]

        with patch.object(atlas, 'wait_until'), patch.object(atlas, 'p9_marks', side_effect=marks):
            result = atlas.measure_p9(page, 'http://127.0.0.1', lambda ok, msg: self.assertTrue(ok, msg))
        self.assertEqual([r['layout_plus_paint_ms'] for r in result['runs']], [45, 70, 40, 48, 42])
        self.assertEqual(result['median_layout_plus_paint_ms'], 45)
        self.assertEqual(result['budget_ms'], 50)
        self.assertEqual([c.args[0] for c in page.goto.call_args_list],
                         [f'http://127.0.0.1/?transport=mock&run={i}#/atlas' for i in range(6)])

    def test_p9_requires_marks_for_2500_nodes(self):
        marks = [{'name': name, 'ms': 10, 'nodes': 2500, 'cells': 100} for name in ('layout', 'first-paint')]
        with patch.object(atlas, 'wait_until'), patch.object(atlas, 'p9_marks', return_value=marks):
            with self.assertRaises(AssertionError):
                atlas.measure_p9(Mock(), 'http://127.0.0.1', lambda ok, msg: self.assertTrue(ok, msg))


if __name__ == '__main__':
    unittest.main()
