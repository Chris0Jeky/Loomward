import copy
import json
import unittest
from pathlib import Path

try:
    import jsonschema
except ImportError:  # the CI Python legs run without it, as for the other contract tests
    jsonschema = None

V3 = Path(__file__).resolve().parents[1] / 'contracts/v3'


class AppsDesktopContracts(unittest.TestCase):
    def setUp(self):
        if jsonschema is None:
            self.skipTest('jsonschema not installed')

    def test_read_only_pages_and_unknowns(self):
        commands = json.loads((V3 / 'commands.json').read_text())['commands']
        defs = json.loads((V3 / 'view-service.schema.json').read_text())['$defs']
        for command, rows in [('apps.list', 'applications'), ('desktop.contexts', 'shortcuts')]:
            spec = commands[command]
            self.assertEqual(spec['effects'], 'none')
            for side in ['request', 'result']:
                value = json.loads((V3 / f'examples/commands/{command}.{side}.json').read_text())
                validator = jsonschema.Draft202012Validator({'$defs': defs, '$ref': f"#/$defs/{spec[side]}"})
                validator.validate(value)
                invalid = copy.deepcopy(value)
                if side == 'request':
                    invalid['limit'] = 201
                else:
                    invalid[rows] *= 201
                self.assertFalse(validator.is_valid(invalid))
            request = json.loads((V3 / f'examples/commands/{command}.request.json').read_text())
            request['path'] = 'invented'
            validator = jsonschema.Draft202012Validator({'$defs': defs, '$ref': f"#/$defs/{spec['request']}"})
            self.assertFalse(validator.is_valid(request))
        apps = json.loads((V3 / 'examples/commands/apps.list.result.json').read_text())
        self.assertIsNone(apps['applications'][1]['estimated_size_bytes'])
        self.assertTrue(apps['applications'][1]['size_reason'])
        desktop = json.loads((V3 / 'examples/commands/desktop.contexts.result.json').read_text())
        self.assertEqual(desktop['shortcuts'][0]['health'], 'missing')


    def test_apps_list_sort_is_optional_and_closed(self):
        commands = json.loads((V3 / 'commands.json').read_text())['commands']
        defs = json.loads((V3 / 'view-service.schema.json').read_text())['$defs']
        self.assertEqual(commands['apps.list']['request'], 'AppsListRequest')
        apps = jsonschema.Draft202012Validator({'$defs': defs, '$ref': '#/$defs/AppsListRequest'})
        desktop = jsonschema.Draft202012Validator({'$defs': defs, '$ref': '#/$defs/MetadataPageRequest'})
        for sort in ('registry', 'size_desc'):
            self.assertTrue(apps.is_valid({'limit': 200, 'cursor': None, 'sort': sort}))
        self.assertTrue(apps.is_valid({'limit': 200, 'cursor': None}))
        for bad in ('name', 'SIZE_DESC', None, 1):
            self.assertFalse(apps.is_valid({'limit': 200, 'cursor': None, 'sort': bad}), bad)
        self.assertFalse(apps.is_valid({'limit': 200}))
        self.assertFalse(desktop.is_valid({'limit': 200, 'cursor': None, 'sort': 'size_desc'}))
        self.assertEqual(commands['desktop.contexts']['request'], 'MetadataPageRequest')


if __name__ == '__main__':
    unittest.main()
