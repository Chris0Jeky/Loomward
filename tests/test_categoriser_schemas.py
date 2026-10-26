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

    def test_schema_rejects_invalid_packet_fields(self):
        for pointer, value in [
                (('unexpected',), True),
                (('shape', 'unexpected'), True),
                (('subject', 'name_tokens'), ['c:\\x']),
                (('subject', 'name_tokens'), ['a/b']),
                (('subject', 'name_tokens'), ['two words']),
                (('extensions', 0, 'ext'), 'a.b'),
                (('packet_id',), 'ABC')]:
            with self.subTest(pointer=pointer, value=value):
                record = copy.deepcopy(self.records[0])
                target = record['packet']
                for key in pointer[:-1]:
                    target = target[key]
                target[pointer[-1]] = value
                self.assertFalse(self.packet_validator.is_valid(record['packet']))
                self.assertFalse(self.record_validator.is_valid(record))
        record = copy.deepcopy(self.records[0])
        del record['packet']['coverage']
        self.assertFalse(self.record_validator.is_valid(record))
        record = copy.deepcopy(self.records[0])
        record['unexpected'] = True
        self.assertFalse(self.record_validator.is_valid(record))

    def test_all_patterns_reject_trailing_newlines(self):
        for pointer, value in [
                (('packet', 'packet_id'), 'abc\n'),
                (('packet', 'group_id'), 'abc\n'),
                (('record_id',), 'abc\n'),
                (('packet', 'subject', 'name_tokens'), ['abc\n']),
                (('packet', 'extensions', 0, 'ext'), 'rs\n'),
                (('summary_facts',), ['/shape/files\n'])]:
            with self.subTest(pointer=pointer):
                record = copy.deepcopy(self.records[0])
                target = record
                for key in pointer[:-1]:
                    target = target[key]
                target[pointer[-1]] = value
                self.assertFalse(self.record_validator.is_valid(record))

    def test_tokens_reject_controls_and_bidi_characters(self):
        codepoints = [*range(32), *range(127, 160), *range(0x202a, 0x202f),
                      *range(0x2066, 0x206a), 0x200e, 0x200f]
        for codepoint in codepoints:
            with self.subTest(codepoint=codepoint):
                packet = copy.deepcopy(self.records[0]['packet'])
                packet['subject']['name_tokens'] = ['a' + chr(codepoint) + 'b']
                self.assertFalse(self.packet_validator.is_valid(packet))

    def test_labels_are_unique_per_axis_within_each_provenance(self):
        for provenance in ['human_labels', 'teacher_labels']:
            for axis, labels in [('meaning', ['archive', 'download']),
                                 ('lifecycle', ['active', 'dormant']),
                                 ('recreatability', ['unique', 'mixed'])]:
                with self.subTest(provenance=provenance, axis=axis):
                    record = copy.deepcopy(self.records[0])
                    record[provenance] = [{'axis': axis, 'label': label}
                                          for label in labels]
                    self.assertFalse(self.record_validator.is_valid(record))
        record = copy.deepcopy(self.records[0])
        record['human_labels'] = []
        record['teacher_labels'] = []
        self.record_validator.validate(record)

    def test_unobserved_coverage_requires_unknown_facts(self):
        for state in ['denied', 'unscanned', 'excluded', 'cancelled', 'unknown']:
            packet = copy.deepcopy(self.records[0]['packet'])
            packet['coverage'].update(state=state, allocation_known=False)
            for key in ['files', 'folders', 'logical_bytes', 'allocated_bytes']:
                packet['shape'][key] = None
            packet['duplicates'] = {'duplicate_bytes': None, 'groups': None}
            packet['families'] = []
            packet['extensions'] = []
            packet['media'] = None
            self.packet_validator.validate(packet)
            for section, key, value in [
                    ('shape', 'files', 0), ('shape', 'folders', 0),
                    ('shape', 'logical_bytes', 0), ('shape', 'allocated_bytes', 0),
                    ('duplicates', 'duplicate_bytes', 0), ('duplicates', 'groups', 0),
                    (None, 'families', self.records[0]['packet']['families']),
                    (None, 'extensions', self.records[0]['packet']['extensions']),
                    (None, 'media', self.records[6]['packet']['media'])]:
                with self.subTest(state=state, section=section, key=key):
                    invalid = copy.deepcopy(packet)
                    target = invalid[section] if section else invalid
                    target[key] = value
                    self.assertFalse(self.packet_validator.is_valid(invalid))

    def test_allocation_presence_matches_coverage(self):
        packet = copy.deepcopy(self.records[0]['packet'])
        packet['shape']['allocated_bytes'] = None
        self.assertFalse(self.packet_validator.is_valid(packet))
        packet['coverage']['allocation_known'] = False
        self.packet_validator.validate(packet)
        packet['shape']['allocated_bytes'] = 0
        self.assertFalse(self.packet_validator.is_valid(packet))

    def test_denied_fixture_and_extension_totals(self):
        denied = next(record['packet'] for record in self.records
                      if record['packet']['coverage']['state'] == 'denied')
        self.assertTrue(all(denied['shape'][key] is None for key in
                            ['files', 'folders', 'logical_bytes', 'allocated_bytes']))
        self.assertEqual(denied['duplicates'], {'duplicate_bytes': None, 'groups': None})
        self.assertEqual(denied['families'], [])
        self.assertEqual(denied['extensions'], [])
        self.assertIsNone(denied['media'])
        for record in self.records:
            packet = record['packet']
            if packet['shape']['files'] is not None:
                self.assertLessEqual(sum(ext['files'] for ext in packet['extensions']),
                                     packet['shape']['files'])

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
