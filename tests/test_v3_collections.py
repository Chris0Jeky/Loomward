"""Collection contract regressions, using only synthetic entries."""
import json
import unittest
from pathlib import Path

V3 = Path(__file__).resolve().parents[1] / 'contracts/v3'


class CollectionContractTests(unittest.TestCase):
    def validator(self, name):
        try:
            import jsonschema
        except ImportError:
            self.skipTest('jsonschema not installed')
        schema = json.loads((V3 / 'view-service.schema.json').read_text(encoding='utf-8'))
        return jsonschema.Draft202012Validator({'$defs': schema['$defs'], '$ref': f'#/$defs/{name}'})

    def test_resolved_requires_entry_and_unavailable_requires_null(self):
        validator = self.validator('CollectionMember')
        entry = json.loads((V3 / 'examples/commands/collections.members.result.json').read_text(encoding='utf-8'))['items'][0]['entry']
        for status in ['resolved', 'unresolved', 'root_revoked', 'root_unavailable']:
            for value in [entry, None]:
                with self.subTest(status=status, entry=value is not None):
                    self.assertEqual(validator.is_valid({'member_id': 'om_1', 'status': status, 'entry': value}), (status == 'resolved') == (value is not None))

    def test_names_reject_all_unicode_controls(self):
        validator = self.validator('CollectionCreateRequest')
        for point in [*range(0x20), *range(0x7f, 0xa0)]:
            with self.subTest(point=point):
                self.assertFalse(validator.is_valid({'name': f'Projects{chr(point)}'}))
        self.assertTrue(validator.is_valid({'name': 'Projects 日本語'}))

    def test_remove_members_is_optional_unique_bounded_and_nonnullable(self):
        validator = self.validator('CollectionMembersUpdate')
        payload = {'collection_id': 'co_1', 'add': [], 'remove': []}
        self.assertTrue(validator.is_valid(payload))
        for members, valid in [([], True), (['om_1'], True), (['om_1', 'om_1'], False), (None, False), ([f'om_{i}' for i in range(1, 502)], False)]:
            with self.subTest(members=members):
                self.assertEqual(validator.is_valid({**payload, 'remove_members': members}), valid)


if __name__ == '__main__':
    unittest.main()
