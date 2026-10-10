import json
import unittest
import urllib.request
from unittest import mock

from loomward.teacher import request_teacher, strict_json


def nested_list(depth):
    return '[' * depth + ']' * depth


def wrap_nested(depth):
    value = []  # already one level deep
    for _ in range(depth - 1):
        value = [value]
    return value


class TeacherNestingTests(unittest.TestCase):
    def test_nesting_beyond_32_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'nest'):
            strict_json(nested_list(33))

    def test_extreme_nesting_fails_closed(self):
        with self.assertRaisesRegex(ValueError, 'nest'):
            strict_json(nested_list(5000))

    def test_nesting_at_32_still_parses(self):
        self.assertEqual(strict_json(nested_list(32)), wrap_nested(32))


class TeacherTimeoutTests(unittest.TestCase):
    def call(self, timeout):
        return request_teacher(
            'http://127.0.0.1:1/v1/chat/completions',
            'test-model', 'item-1', {'name': 'invoice.pdf'},
            ['Finance', 'Documents'],
            consent_metadata=True, timeout=timeout)

    def test_invalid_timeouts_rejected_before_network(self):
        for bad in (0, -1, 0.05, 0.09, 121, 120.5, True, False,
                    '60', None, [60], float('nan'), float('inf')):
            with self.subTest(timeout=bad):
                with mock.patch.object(urllib.request, 'build_opener') as opener:
                    with self.assertRaisesRegex(ValueError, 'timeout'):
                        self.call(bad)
                    opener.assert_not_called()

    def test_valid_timeout_boundaries_reach_transport(self):
        decision = {'item_id': 'item-1', 'label': 'Finance', 'reason': 'Invoice name',
                    'evidence': ['name'], 'abstain': False}
        body = json.dumps({'choices': [{'message': {'content': json.dumps(decision)}}]}).encode()
        for good in (0.1, 60, 120, 5):
            with self.subTest(timeout=good):
                response = mock.MagicMock()
                response.read.return_value = body
                response.__enter__.return_value = response
                with mock.patch.object(urllib.request, 'build_opener') as opener:
                    opener.return_value.open.return_value = response
                    result = self.call(good)
                self.assertEqual(result['source'], 'teacher')
                opener.assert_called_once()
