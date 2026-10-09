"""Check checked-in interchange contracts against executable reference definitions."""
import json
import unittest
from pathlib import Path
from loomward.catalog import Catalog
from loomward.interop import ToolService, SCHEMAS, OUTPUT_SCHEMA
ROOT = Path(__file__).resolve().parents[1]
CONTRACTS = ROOT / 'contracts/v2'

class ContractTests(unittest.TestCase):
    def read_contract(self, name):
        path = CONTRACTS / name
        self.assertTrue(path.is_file(), f'Missing versioned contract: {name}')
        return json.loads(path.read_text(encoding='utf-8'))
    def test_tool_inputs_match_runtime(self):
        for name, schema in SCHEMAS.items():
            exported = self.read_contract(name + '.input.schema.json')
            exported.pop('$schema'); exported.pop('title')
            self.assertEqual(exported, schema)
    def test_output_contract_matches_runtime(self):
        exported = self.read_contract('tool-output.schema.json')
        exported.pop('$schema'); exported.pop('title')
        self.assertEqual(exported, OUTPUT_SCHEMA)
    def test_launch_profiles_do_not_advertise_extra_authority(self):
        manifest = self.read_contract('reference-capabilities.json')
        self.assertEqual(manifest['status'], 'executable_reference_not_host_certified')
        inventory = json.loads((ROOT/'fixtures/demo.json').read_text())['inventory']
        for profile, grant in [('summary',False),('names',True)]:
            with Catalog(inventory, disclose_names=grant) as c:
                self.assertEqual(manifest['profiles'][profile]['tools'], [t['name'] for t in ToolService(c).tool_specs()])
            self.assertFalse(manifest['profiles'][profile]['approve'])
            self.assertFalse(manifest['profiles'][profile]['execute'])
    def test_examples_are_current_and_explicitly_synthetic(self):
        modern = self.read_contract('modern-requests.json')
        self.assertEqual(modern['origin'], 'synthetic_examples')
        self.assertEqual(len(modern['requests']),3)
        for row in modern['requests']:
            self.assertEqual(row['params']['_meta']['io.modelcontextprotocol/protocolVersion'],'2026-07-28')
        legacy = self.read_contract('legacy-requests.json')
        self.assertEqual(legacy['requests'][0]['params']['protocolVersion'],'2025-11-25')
    def test_future_contracts_are_not_claimed_as_runtime(self):
        index = self.read_contract('INDEX.json')
        for name in ('provider-manifest.schema.json','event-envelope.schema.json','proposal-record.schema.json'):
            self.assertEqual(index['contracts'][name]['status'], 'design_only')
            self.assertTrue(self.read_contract(name)['$schema'].endswith('2020-12/schema'))
    def test_draft_manifests_do_not_contain_credentials(self):
        example = self.read_contract('provider-manifest.example.json')
        self.assertEqual(example['requested_capabilities'], ['observe.summary'])
        self.assertEqual(example['execution'], 'unconfigured')
        self.assertFalse(example['installed'])
        self.assertNotIn('token', json.dumps(example).lower())
    def test_json_schema_examples_validate_when_validator_present(self):
        try:
            from jsonschema import Draft202012Validator
        except ImportError:
            self.skipTest('Optional JSON Schema validator is unavailable; structure/parity tests still run')
        index = self.read_contract('INDEX.json')
        for name, info in index['contracts'].items():
            if not name.endswith('.schema.json'): continue
            schema = self.read_contract(name)
            Draft202012Validator.check_schema(schema)
            for example in info.get('examples',[]):
                Draft202012Validator(schema).validate(self.read_contract(example))
        demo=json.loads((ROOT/'fixtures/demo.json').read_text())['inventory']
        with Catalog(demo) as c:
            result=ToolService(c).call('workspace_summary',{})
            Draft202012Validator(self.read_contract('tool-output.schema.json')).validate(result)

if __name__ == '__main__': unittest.main()
