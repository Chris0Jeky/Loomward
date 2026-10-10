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

    def test_commands_and_events_name_defined_types(self):
        for kind in ('commands', 'events'):
            for name, spec in self.commands[kind].items():
                for key in ('request', 'result', 'data'):
                    if isinstance(spec, dict) and isinstance(spec.get(key), str):
                        self.assertIn(spec[key], self.defs, f'{kind} {name}.{key}')

    def test_no_command_declares_an_effect(self):
        for name, spec in self.commands['commands'].items():
            self.assertEqual(spec.get('effects', 'none'), 'none', name)
            self.assertNotRegex(name, r'(delete|move|kill|suspend|trim|apply|execute|uninstall)')


if __name__ == '__main__':
    unittest.main()
