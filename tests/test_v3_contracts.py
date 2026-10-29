"""Structural checks for the v3 view-service contract until the protocol crate's own tests land."""
import json
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
V3 = ROOT / 'contracts/v3'


def load(name):
    return json.loads((V3 / name).read_text(encoding='utf-8'))


class V3ContractTests(unittest.TestCase):
    def setUp(self):
        self.schema = load('view-service.schema.json')
        self.defs = self.schema['$defs']
        self.commands = load('commands.json')

    def test_project_discovery_is_explicit_bounded_and_read_only(self):
        self.assertTrue('projects.find' in self.commands['commands'],
                        'An explicit catalogue-only project discovery command is missing')
        command = self.commands['commands']['projects.find']
        self.assertEqual(command['effects'], 'none')
        self.assertEqual(command['adapters'], ['tauri', 'http'])
        self.assertNotIn('client_result', command)
        import jsonschema
        validator = jsonschema.Draft202012Validator({
            '$defs': self.defs, '$ref': '#/$defs/' + command['request']})
        request = {'root_id': 'rt_1', 'limit': 50, 'cursor': None}
        validator.validate(request)
        for limit in (0, 201):
            self.assertFalse(validator.is_valid(dict(request, limit=limit)))
        self.assertFalse(validator.is_valid(dict(request, path='Synthetic project')))
        self.assertFalse(validator.is_valid({'limit': 50, 'cursor': None}))

    def test_schema_is_valid_json_schema(self):
        try:
            import jsonschema
        except ImportError:
            self.skipTest('jsonschema not installed')
        jsonschema.validators.validator_for(self.schema).check_schema(self.schema)

    def test_every_internal_reference_resolves(self):
        text = (V3 / 'view-service.schema.json').read_text(encoding='utf-8')
        missing = set(re.findall(r'#/\$defs/([A-Za-z0-9_]+)', text)) - set(self.defs)
        self.assertEqual(missing, set())

    def test_job_counter_fixtures_match_the_schema(self):
        try:
            import jsonschema
        except ImportError:
            self.skipTest('jsonschema not installed')
        schema = dict(self.schema, **{'$ref': '#/$defs/Job'})
        schema.pop('anyOf', None)
        validator = jsonschema.validators.validator_for(schema)(schema)
        for path in (V3 / 'examples/commands').glob('*.result.json'):
            result = json.loads(path.read_text(encoding='utf-8'))
            jobs = result.get('jobs', [result['job']] if 'job' in result else [])
            for job in jobs:
                validator.validate(job)
        validator.validate(load('examples/events/job.state.json')['job'])
        for field in ('seq', 'rev'):
            invalid = load(f'examples/invalid/job-missing-{field}.json')
            self.assertFalse(validator.is_valid(invalid['value']))

    def test_commands_and_events_name_defined_types(self):
        for kind in ('commands', 'events'):
            for name, spec in self.commands[kind].items():
                for key in ('request', 'result', 'data'):
                    if isinstance(spec, dict) and isinstance(spec.get(key), str):
                        self.assertIn(spec[key], self.defs, f'{kind} {name}.{key}')

    def test_copy_receipt_vocabulary_is_additive_and_truthful(self):
        receipt = load('examples/commands/copy.status.result.json')['receipt']
        self.assertIsNone(receipt['highest_label'])
        self.assertEqual(receipt['state'], 'unverified')
        self.assertIn('not power-loss tested on this disk', receipt['explanation'])
        self.assertNotIn('durable', json.dumps(receipt).lower())
        self.assertNotIn('receipt', self.defs['CopyStatus']['required'])
        self.assertTrue(self.defs['CopyStatus']['properties']['items_verified']['deprecated'])

    def test_copy_status_accepts_bounded_disposition_without_native_identity(self):
        try:
            import jsonschema
        except ImportError:
            self.skipTest('jsonschema not installed')
        status = load('examples/commands/copy.status.result.json')
        status['disposition'] = {
            'explanation': 'Cancelled after source_in_use. Journal evidence remains.',
            'items_total': 3, 'items_verified': 2, 'items_unverified': 1,
            'refusal_codes': ['source_in_use'], 'stop_cause': 'source_in_use',
            'resumable_before_cancel': True,
            'resume_possible': False,
        }
        validator = jsonschema.Draft202012Validator({
            '$defs': self.defs, '$ref': '#/$defs/CopyStatus'})
        validator.validate(status)
        for field, value in [('explanation', 'x' * 2049),
                             ('refusal_codes', ['source_in_use'] * 33)]:
            changed = dict(status, disposition=dict(status['disposition'], **{field: value}))
            self.assertFalse(validator.is_valid(changed))
        for cause in ('emergency_stop', 'cancelled', 'grant_revoked'):
            validator.validate(dict(status, disposition=dict(status['disposition'], stop_cause=cause)))
        self.assertFalse(validator.is_valid(
            dict(status, disposition=dict(status['disposition'], stop_cause='journaled'))))

    def test_cold_learning_queue_has_required_null_model_ids(self):
        try:
            import jsonschema
        except ImportError:
            self.skipTest('jsonschema not installed')
        queue = load('examples/commands/learning.queue.result.json')
        queue['model_id'] = None
        suggestion = queue['items'][0]['suggestion']
        suggestion.update(model_id=None, ranked=[], abstain=True,
                          reasons=['no_training_examples'], teacher=None)
        validator = jsonschema.Draft202012Validator({
            '$defs': self.defs, '$ref': '#/$defs/LearningQueue'})
        validator.validate(queue)
        del suggestion['model_id']
        self.assertFalse(validator.is_valid(queue))

    def test_duplicate_examples_validate_requests_and_terminal_evidence(self):
        queued = load('examples/commands/duplicates.inspect.result.json')['job']
        self.assertIsNone(queued['started_at'])
        terminal = load('examples/dtos/duplicates-result.json')['value']['job']
        self.assertLessEqual(terminal['created_at'], terminal['started_at'])
        self.assertLessEqual(terminal['started_at'], terminal['finished_at'])
        try:
            import jsonschema
        except ImportError:
            self.skipTest('jsonschema not installed')
        for name in ('duplicates.inspect', 'duplicates.cancel'):
            for side, key in (('request', 'request'), ('result', 'result')):
                schema = {'$defs': self.defs, '$ref': f"#/$defs/{self.commands['commands'][name][key]}"}
                jsonschema.Draft202012Validator(schema).validate(load(f'examples/commands/{name}.{side}.json'))
        result = load('examples/dtos/duplicates-result.json')['value']
        validator = jsonschema.Draft202012Validator({'$defs': self.defs, '$ref': '#/$defs/JobResult'})
        validator.validate(result)
        self.assertEqual(result['job']['duplicates']['links'], [[2, 3]])
        result['job']['duplicates']['groups'] = [[0, 0]]
        self.assertFalse(validator.is_valid(result))

    def test_no_command_declares_an_effect(self):
        # One open exception (docs/42 ADR-V3-24): Loomward's own previewed bundle, written to a file the
        # owner picks in a Rust-side save dialog. The copy effects (docs/47) are declared but answer
        # capability_unavailable until lane C8. Every other command has no effect.
        gated = {'diagnostics.save': 'owner_picked_export', 'copy.start': 'copy_and_verify',
                 'copy.cancel': 'copy_and_verify', 'copy.abandon': 'copy_and_verify'}
        for name, spec in self.commands['commands'].items():
            self.assertEqual(spec.get('effects', 'none'), gated.get(name, 'none'), name)
            self.assertNotRegex(name, r'(delete|move|kill|suspend|trim|apply|execute|uninstall)')


if __name__ == '__main__':
    unittest.main()
