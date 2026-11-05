"""Coverage phase snapshots remain optional and never invent work totals (#1108)."""
import json
import unittest
from pathlib import Path

V3 = Path(__file__).resolve().parents[1] / 'contracts/v3'


class CoverageProgressContractTests(unittest.TestCase):
    def setUp(self):
        try:
            import jsonschema
        except ImportError:
            self.skipTest('jsonschema not installed')
        schema = json.loads((V3 / 'view-service.schema.json').read_text(encoding='utf-8'))
        self.validator = jsonschema.Draft202012Validator({
            '$defs': schema['$defs'], '$ref': '#/$defs/Job'})
        self.job = json.loads((V3 / 'examples/commands/jobs.get.result.json').read_text(encoding='utf-8'))['job']
        self.job.update(kind='coverage', state='running', progress=None)

    def test_older_jobs_omit_progress_and_new_jobs_name_only_observed_phases(self):
        self.validator.validate(self.job)
        for phase in ('listing_destination', 'hashing_source', 'hashing_destination', 'rechecking_source'):
            with self.subTest(phase=phase):
                self.validator.validate({**self.job, 'coverage_progress': {'phase': phase}})

    def test_progress_rejects_null_missing_unknown_phases_and_invented_totals(self):
        for progress in (None, {}, {'phase': 'done'}, {'phase': None},
                         {'phase': 'hashing_source', 'percent': 50},
                         {'phase': 'hashing_source', 'files_total': 0}):
            with self.subTest(progress=progress):
                self.assertFalse(self.validator.is_valid({**self.job, 'coverage_progress': progress}))


if __name__ == '__main__':
    unittest.main()
