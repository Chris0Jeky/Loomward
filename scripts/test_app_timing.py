"""Small regression checks for browser gate sampling and asynchronous server assertions."""
import contextlib
import importlib.util
import io
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[1]
import test_app
import test_app_views

spec = importlib.util.spec_from_file_location('atlas_e2e', ROOT / 'app/tests/e2e/test_atlas.py')
atlas = importlib.util.module_from_spec(spec)
spec.loader.exec_module(atlas)


class AppE2ETimingTests(unittest.TestCase):
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
