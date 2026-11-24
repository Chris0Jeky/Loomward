"""Contract tests for the synthetic categoriser corpus."""
import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

from scripts import categoriser_synth as synth


class SyntheticCategoriserContracts(unittest.TestCase):
    def test_empty_count_returns_empty_list(self):
        self.assertEqual(synth.generate_records(0, 1), [])

    def test_negative_count_raises(self):
        with self.assertRaises(ValueError):
            synth.generate_records(-1, 1)

    def test_each_lineage_has_three_distinct_packets(self):
        records = synth.generate_records(6, 1)
        groups = [record['packet']['group_id'] for record in records]
        self.assertEqual(groups, [groups[0]] * 3 + [groups[3]] * 3)
        self.assertNotEqual(groups[0], groups[3])
        self.assertEqual(len({record['packet']['packet_id'] for record in records}), 6)

    def test_sparse_sibling_is_partial_with_unknown_labels(self):
        record = synth.generate_records(3, 1)[1]
        packet = record['packet']
        self.assertEqual(packet['markers'], [])
        self.assertEqual(packet['coverage']['state'], 'partial')
        self.assertIs(packet['coverage']['allocation_known'], False)
        self.assertIs(packet['coverage']['link_counts_observed'], False)
        self.assertIsNone(packet['shape']['logical_bytes'])
        self.assertIsNone(packet['shape']['allocated_bytes'])
        for extension in packet['extensions']:
            self.assertIsNone(extension['bytes'])
        for family in packet['families']:
            self.assertIsNone(family['share_bytes'])
        labels = {item['axis']: item['label'] for item in record['human_labels']}
        self.assertEqual(labels, dict.fromkeys(synth.AXES, 'unknown'))

    def test_integer_type_rejects_bool(self):
        with self.assertRaises(ValueError):
            synth.check_structure(True, {'type': 'integer'})
        synth.check_structure(1, {'type': 'integer'})
        synth.check_structure(True, {'type': 'boolean'})

    def test_duplicate_teacher_axis_rejected(self):
        record = synth.generate_records(1, 1)[0]
        record['teacher_labels'] = [{'axis': 'meaning', 'label': 'archive'}]
        synth.validate_record(record)
        record['teacher_labels'].append({'axis': 'meaning', 'label': 'download'})
        with self.assertRaises(ValueError):
            synth.validate_record(record)

    def test_main_writes_records_to_new_subdirectory(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / 'nested' / 'records.json'
            with redirect_stdout(io.StringIO()):
                self.assertEqual(synth.main(['--count', '2', '--seed', '1',
                                             '--out', str(out)]), 0)
            records = json.loads(out.read_text(encoding='utf-8'))
            self.assertEqual(len(records), 2)
            for record in records:
                synth.validate_record(record)


if __name__ == '__main__':
    unittest.main()
