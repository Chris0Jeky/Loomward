"""Regression tests for the planner/learning fix (issue #3).

1. plan_tiers_v2 must accept a scenario without a 'groups' key, exactly as v1
   plan_tiers does (missing groups == empty-group plan), instead of raising
   KeyError.
2. Student.to_dict must return an independent copy: mutating the returned
   dict must not corrupt the model, and from_dict(to_dict()) must round-trip
   identically.
"""
import unittest
from loomward.learning import Student
from loomward.planner import plan_tiers
from loomward.planner_v2 import plan_tiers_v2
from test_planner import scenario


def examples():
    return [
        {'event_id': 'a-human-1', 'item_id': 'a', 'revision': 1, 'source': 'human',
         'label': 'Finance', 'features': {'name': 'invoice payment account', 'extension': '.txt',
                                          'context': '', 'size_bytes': 100}, 'retracted': False},
        {'event_id': 'b-human-1', 'item_id': 'b', 'revision': 1, 'source': 'human',
         'label': 'Finance', 'features': {'name': 'invoice receipt payment', 'extension': '.txt',
                                          'context': '', 'size_bytes': 100}, 'retracted': False},
        {'event_id': 'c-human-1', 'item_id': 'c', 'revision': 1, 'source': 'human',
         'label': 'Media', 'features': {'name': 'photo portrait camera', 'extension': '.txt',
                                        'context': '', 'size_bytes': 100}, 'retracted': False},
        {'event_id': 'd-human-1', 'item_id': 'd', 'revision': 1, 'source': 'human',
         'label': 'Media', 'features': {'name': 'photo image camera', 'extension': '.txt',
                                        'context': '', 'size_bytes': 100}, 'retracted': False},
    ]


class MissingGroupsTests(unittest.TestCase):
    def test_v2_accepts_scenario_without_groups_like_v1(self):
        s = scenario()
        del s['groups']
        expected = plan_tiers(s)
        result = plan_tiers_v2(s)
        self.assertEqual(result['proposals'], [])
        self.assertEqual(result['shortfall_bytes'], expected['shortfall_bytes'])
        self.assertEqual(result['satisfied'], expected['satisfied'])


class StudentToDictTests(unittest.TestCase):
    def test_to_dict_returns_independent_copy(self):
        s = Student(['Finance', 'Media'])
        s.fit(examples())
        before_predict = s.predict({'name': 'invoice payment'})
        before_dict = s.to_dict()
        leaked = s.to_dict()
        leaked['labels'].append('Injected')
        leaked['events'].append({'event_id': 'x', 'item_id': 'x', 'revision': 1,
                                 'source': 'human', 'label': 'Injected',
                                 'features': {'name': 'x', 'extension': '', 'context': '',
                                              'size_bytes': 0}, 'retracted': False})
        leaked['events'][0]['features']['name'] = 'MUTATED'
        self.assertEqual(s.to_dict(), before_dict)
        # The corrupted pre-fix model raised KeyError here on the unknown label.
        self.assertEqual(s.predict({'name': 'invoice payment'}), before_predict)

    def test_from_dict_roundtrip_is_identical(self):
        s = Student(['Finance', 'Media'])
        s.fit(examples())
        restored = Student.from_dict(s.to_dict())
        self.assertEqual(restored.to_dict(), s.to_dict())
        self.assertEqual(restored.predict({'name': 'invoice'}),
                         s.predict({'name': 'invoice'}))


if __name__ == '__main__':
    unittest.main()
