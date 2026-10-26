import copy
import json
import unittest
from pathlib import Path

try:
    import jsonschema
except ImportError:
    jsonschema = None

ROOT = Path(__file__).resolve().parents[1]


class CategoriserSchemas(unittest.TestCase):
    def setUp(self):
        if jsonschema is None:
            self.skipTest('jsonschema not installed')
        from referencing import Registry, Resource

        packet_schema = json.loads(
            (ROOT / 'schemas/evidence-packet-v1.json').read_text(encoding='utf-8'))
        record_schema = json.loads(
            (ROOT / 'schemas/categoriser-record-v1.json').read_text(encoding='utf-8'))
        jsonschema.Draft202012Validator.check_schema(packet_schema)
        jsonschema.Draft202012Validator.check_schema(record_schema)
        registry = Registry().with_resource(
            'evidence-packet-v1.json', Resource.from_contents(packet_schema))
        self.packet_validator = jsonschema.Draft202012Validator(packet_schema)
        self.record_validator = jsonschema.Draft202012Validator(
            record_schema, registry=registry)
        self.records = json.loads(
            (ROOT / 'fixtures/categoriser-v1/records.json').read_text(encoding='utf-8'))

    def test_synthetic_fixtures_match_both_schemas(self):
        self.assertGreaterEqual(len(self.records), 10)
        for record in self.records:
            with self.subTest(record_id=record['record_id']):
                self.assertIs(record['synthetic'], True)
                self.assertIs(record['packet']['synthetic'], True)
                self.packet_validator.validate(record['packet'])
                self.record_validator.validate(record)

    def test_record_reference_validates_nested_packet(self):
        record = copy.deepcopy(self.records[0])
        record['packet']['coverage']['state'] = 'invented'
        self.assertFalse(self.packet_validator.is_valid(record['packet']))
        self.assertFalse(self.record_validator.is_valid(record))

    def test_fixture_scenarios_and_separate_provenance(self):
        meanings = {label['label'] for record in self.records
                    for label in record['human_labels'] if label['axis'] == 'meaning'}
        self.assertTrue({'generated_build', 'source_project', 'dependency_install',
                         'package_cache', 'model_weights', 'virtual_disk', 'photo_shoot',
                         'photo_catalogue', 'catalogue_backup', 'installer',
                         'agent_worktree', 'unknown'} <= meanings)
        groups = [record['packet']['group_id'] for record in self.records]
        self.assertLess(len(set(groups)), len(groups))
        self.assertTrue(any(record['teacher_labels'] and
                            record['teacher_labels'][0] not in record['human_labels']
                            for record in self.records))
        partial = next(record['packet'] for record in self.records
                       if record['packet']['coverage']['state'] == 'partial')
        self.assertIsNone(partial['shape']['logical_bytes'])
        self.assertIsNone(partial['shape']['allocated_bytes'])
        self.assertIs(partial['coverage']['allocation_known'], False)


if __name__ == '__main__':
    unittest.main()
