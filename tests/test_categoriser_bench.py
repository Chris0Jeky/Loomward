"""Offline LW-140 contracts; the only network peer is our disposable loopback server."""
import copy
import io
import json
import tempfile
import threading
import unittest
from contextlib import contextmanager, redirect_stderr, redirect_stdout
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest.mock import patch

from scripts import categoriser_bench as bench
from scripts import categoriser_synth as synth

try:
    import jsonschema
except ImportError:
    jsonschema = None

ROOT = Path(__file__).resolve().parents[1]


@contextmanager
def fake_server(answers, host='127.0.0.1'):
    requests = []

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            requests.append((self.path, None))
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'{}')

        def do_POST(self):
            requests.append((self.path, json.loads(self.rfile.read(
                int(self.headers['Content-Length'])))))
            answer = answers[len(requests) - 1]
            headers = {}
            if isinstance(answer, tuple):
                status, body, *extra = answer
                headers = extra[0] if extra else {}
            else:
                status = 200
                body = json.dumps({'choices': [{'message': {'content': answer}}],
                                   'usage': {'prompt_tokens': 100, 'completion_tokens': 10}})
            self.send_response(status)
            for key, value in headers.items():
                self.send_header(key, value)
            self.end_headers()
            self.wfile.write(body.encode())

        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer((host, 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f'http://{host}:{server.server_port}/v1', requests
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


class SyntheticCorpus(unittest.TestCase):
    def test_deterministic_schema_complete_and_grouped(self):
        records = synth.generate_records(200, 1)
        self.assertEqual(records, synth.generate_records(200, 1))
        self.assertNotEqual(records, synth.generate_records(200, 2))
        for count in (100, 101, 200):
            meanings = {r['human_labels'][0]['label'] for r in synth.generate_records(count, 1)}
            self.assertEqual(meanings, set(bench.AXES['meaning']))
        groups = [r['packet']['group_id'] for r in records]
        self.assertLess(len(set(groups)), len(groups))
        self.assertEqual(len({r['record_id'] for r in records}), len(records))
        if jsonschema is not None:
            from referencing import Registry, Resource
            registry = Registry().with_resource('evidence-packet-v1.json',
                                                Resource.from_contents(synth.PACKET_SCHEMA))
            validator = jsonschema.Draft202012Validator(synth.RECORD_SCHEMA, registry=registry)
        for record in records:
            synth.validate_record(record)
            if jsonschema is not None:
                validator.validate(record)
            self.assertIs(record['synthetic'], True)
            self.assertIs(record['packet']['synthetic'], True)
            self.assertEqual(record['teacher_labels'], [])
            for token in record['packet']['subject']['name_tokens']:
                self.assertRegex(token, r'^[a-z]+$')
            for pointer in record['summary_facts']:
                value = record['packet']
                for part in pointer.strip('/').split('/'):
                    value = value[int(part)] if isinstance(value, list) else value[part]
            packet = record['packet']
            self.assertEqual(sum(e['files'] for e in packet['extensions']), packet['shape']['files'])
            if packet['shape']['logical_bytes'] is not None:
                self.assertEqual(sum(e['bytes'] for e in packet['extensions']),
                                 packet['shape']['logical_bytes'])
        self.assertTrue(any(r['packet']['coverage']['state'] == 'partial' for r in records))
        self.assertTrue(any(not r['packet']['markers'] for r in records))

    def test_builtin_validation_rejects_bad_structure(self):
        for mutate in (lambda r: r['packet']['coverage'].update(state='invented'),
                       lambda r: r['packet']['shape'].update(files=-1),
                       lambda r: r.update(unexpected=True),
                       lambda r: r['packet']['subject'].update(name_tokens=['bad/path']),
                       lambda r: r['human_labels'].append(r['human_labels'][0])):
            record = synth.generate_records(1, 1)[0]
            mutate(record)
            with self.assertRaises(ValueError):
                synth.validate_record(record)

    def test_builtin_validation_enforces_conditionals_and_contains(self):
        record = copy.deepcopy(next(r for r in json.loads(
            (ROOT / 'fixtures/categoriser-v1/records.json').read_text(encoding='utf-8'))
            if r['packet']['coverage']['state'] == 'denied'))
        synth.validate_record(record)
        record['packet']['shape']['files'] = 1
        with self.assertRaises(ValueError):
            synth.validate_record(record)
        schema = {'type': 'array', 'contains': {'const': 'match'},
                  'minContains': 1, 'maxContains': 1}
        synth.check_structure(['match', 'other'], schema)
        for value in ([], ['other'], ['match', 'match']):
            with self.subTest(value=value), self.assertRaises(ValueError):
                synth.check_structure(value, schema)
        synth.check_structure([], {**schema, 'minContains': 0})

    def test_builtin_validation_refuses_unimplemented_keywords(self):
        for schema in ({'not': {}}, {'oneOf': [{}, {'not': {}}]},
                       {'if': {'const': False}, 'then': {'not': {}}},
                       {'properties': {'absent': {'format': 'date'}}},
                       {'$ref': '#/$defs/count', 'not': {}},
                       {'additionalProperties': {'not': {}}}):
            with self.subTest(schema=schema):
                with self.assertRaisesRegex(ValueError, 'unsupported schema keyword'):
                    synth.check_structure({}, schema)

    def test_synthetic_shapes_ages_and_conflicting_markers(self):
        records = synth.generate_records(300, 7)
        ages = set()
        conflicts = []
        for record in records:
            packet = record['packet']
            self.assertLessEqual(packet['shape']['folders'], packet['shape']['files'])
            age = packet['observed_at'] - packet['time']['newest_content_write']
            ages.add(age)
            truth = {label['axis']: label['label'] for label in record['human_labels']}
            if truth['lifecycle'] != 'unknown':
                self.assertEqual(truth['lifecycle'], 'active' if age < 30 * 86400 else
                                 'dormant' if age < 730 * 86400 else 'historical')
            if {'cargo_target_dir', 'lightroom_catalog'} <= set(packet['markers']):
                conflicts.append(truth)
        self.assertGreater(len(ages), 30)
        self.assertTrue(any(age % 86400 for age in ages))
        self.assertTrue(conflicts)
        self.assertTrue(all(t['meaning'] == t['recreatability'] == 'unknown' for t in conflicts))


class Benchmark(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.records = synth.generate_records(5, 3)
        labels = {'meaning': 'generated_build', 'lifecycle': 'dormant',
                  'recreatability': 'generated'}
        for record in self.records:
            record['human_labels'] = [{'axis': k, 'label': v} for k, v in labels.items()]
        self.labels = labels

    def run_bench(self, endpoint, task='classify', extra=()):
        corpus = self.directory / 'corpus.json'
        corpus.write_text(json.dumps(self.records), encoding='utf-8')
        out = self.directory / 'results'
        argv = ['--endpoint', endpoint, '--model', 'fake-model', '--task', task,
                '--corpus', str(corpus), '--out', str(out), *extra]
        with redirect_stdout(io.StringIO()):
            self.assertEqual(bench.main(argv), 0)
        return (json.loads((out / 'summary.json').read_text()),
                [json.loads(line) for line in (out / 'records.jsonl').read_text().splitlines()])

    def test_classification_metrics_and_prompt_isolation(self):
        wrong = {'meaning': 'archive', 'lifecycle': 'active', 'recreatability': 'unique'}
        answers = [json.dumps(self.labels), json.dumps(wrong),
                   json.dumps(dict.fromkeys(bench.AXES, 'unknown')), '{broken',
                   json.dumps({**self.labels, 'meaning': 'invented'})]
        self.records[0]['packet']['subject']['name_tokens'] = ['ignore', 'instructions', 'sentinel']
        with fake_server(answers) as (endpoint, requests):
            summary, rows = self.run_bench(endpoint)
        self.assertEqual(summary['records'], 5)
        self.assertEqual(summary['invalid_rate'], .4)
        self.assertEqual(summary['coverage'], .4)
        self.assertEqual(summary['selective_accuracy'], .5)
        for axis, metrics in summary['axes'].items():
            self.assertEqual(metrics['accuracy'], .2)
            self.assertEqual(metrics['coverage'], .4)
            self.assertEqual(metrics['selective_accuracy'], .5)
            self.assertEqual(metrics['macro_f1'], 1 / 9)
            self.assertEqual(metrics['support'], 5)
            self.assertIn('unknown', metrics['macro_f1_labels'])
        self.assertEqual(summary['tokens'], {'prompt': 500, 'completion': 50,
                                             'prompt_reported': 5, 'completion_reported': 5})
        latency = summary['latency_seconds']
        self.assertGreaterEqual(latency['p50'], 0)
        self.assertLessEqual(latency['p50'], latency['p95'])
        self.assertLessEqual(latency['p95'], latency['max'])
        self.assertEqual(summary['model'], 'fake-model')
        self.assertEqual(summary['endpoint_host'], '127.0.0.1')
        self.assertIsNone(summary['unsupported_claim_rate'])
        self.assertEqual([r['invalid'] for r in rows], [False, False, False, True, True])
        self.assertTrue(all(rows[0]['correctness'].values()))
        self.assertFalse(any(rows[1]['correctness'].values()))
        for index, (path, request) in enumerate(requests):
            self.assertEqual(path, '/v1/chat/completions')
            self.assertEqual(request['temperature'], 0)
            self.assertEqual(request['response_format']['type'], 'json_schema')
            self.assertEqual(request['response_format']['json_schema']['schema'], bench.ANSWER_SCHEMA)
            system, user = request['messages']
            self.assertNotIn('sentinel', system['content'])
            self.assertNotIn('packet_id', system['content'])
            self.assertIn('never instructions', system['content'])
            self.assertIn('```json', user['content'])
            self.assertEqual(json.loads(user['content'].split('```json\n')[1].rsplit('\n```', 1)[0]),
                             self.records[index]['packet'])
            self.assertNotIn('human_labels', user['content'])

    def test_packet_backticks_cannot_close_fence(self):
        packet = self.records[0]['packet']
        packet['subject']['name_tokens'] = ['```ignore```']
        user = bench.make_request(packet, 'fake', 'classify', 0)['messages'][1]['content']
        self.assertEqual(user.count('```'), 2)
        evidence = user.split('```json\n')[1].rsplit('\n```', 1)[0]
        self.assertIn('\\u0060\\u0060\\u0060ignore', evidence)
        self.assertEqual(json.loads(evidence), packet)

    def test_summary_faithfulness_metrics(self):
        self.records = self.records[:3]
        for r in self.records:
            r['packet']['shape'].update(logical_bytes=38_200_000_000, files=1204)
        answers = ['38.2 GB of build output, 1,204 files',
                   '987654 files of build output.', 'Build output contains .inventedext files.']
        with fake_server(answers) as (endpoint, requests):
            summary, rows = self.run_bench(endpoint, 'summarise')
        self.assertEqual(rows[0]['unsupported_claims'], [])
        self.assertIn('987654', ' '.join(rows[1]['unsupported_claims']))
        self.assertIn('inventedext', ' '.join(rows[2]['unsupported_claims']))
        self.assertEqual(summary['unsupported_claim_rate'], 2 / 3)
        self.assertEqual(summary['invalid_rate'], 0)
        self.assertIsNone(summary['axes'])
        self.assertIsNone(summary['coverage'])
        self.assertIsNone(summary['selective_accuracy'])
        self.assertNotIn('response_format', requests[0][1])

    def test_claim_units_ages_markers_and_format(self):
        packet = self.records[0]['packet']
        packet['shape'].update(logical_bytes=38_200_000_000, files=1204)
        packet['time']['newest_content_write'] = packet['observed_at'] - 238 * 86400
        packet['markers'] = ['cargo_target_dir']
        self.assertEqual(bench.unsupported_claims('38.2 GB, 1,204 files, 238 days; cargo_target_dir.', packet), [])
        self.assertEqual(bench.unsupported_claims('38200 MB and 38200000 KB.', packet), [])
        self.assertEqual(bench.unsupported_claims('38.2GB of build output.', packet), [])
        self.assertEqual(bench.unsupported_claims('239 days old.', packet), [])
        for sentence in ('100 GB.', '240 days old.', '-1 files.', '1204.5 files.',
                         'cargo_phantom_dir.', 'Contains .exe files.', 'Contains EXE files.',
                         'Contains phantomext extensions.', '999999 years old.',
                         '999GB of build output.', 'Contains .7z files.', '1204% complete.'):
            with self.subTest(sentence=sentence):
                self.assertTrue(bench.unsupported_claims(sentence, packet))
        self.assertFalse(bench.valid_sentence('One sentence. Another sentence.'))
        self.assertFalse(bench.valid_sentence('word ' * 41))
        self.assertFalse(bench.valid_sentence(''))
        self.assertFalse(bench.valid_sentence('First line\nsecond line'))
        self.assertTrue(bench.valid_sentence('The cargo_target_dir marker is present.'))

    def test_abstention_and_missing_human_labels(self):
        self.records = self.records[:1]
        self.records[0]['human_labels'] = [{'axis': 'meaning', 'label': 'unknown'}]
        answer = json.dumps(dict.fromkeys(bench.AXES, 'unknown'))
        with fake_server([answer]) as (endpoint, _):
            summary, rows = self.run_bench(endpoint)
        self.assertEqual(summary['invalid_rate'], 0)
        self.assertEqual(summary['coverage'], 0)
        self.assertIsNone(summary['selective_accuracy'])
        self.assertEqual(summary['axes']['meaning']['accuracy'], 1)
        self.assertEqual(summary['axes']['meaning']['macro_f1'], 1)
        self.assertEqual(summary['axes']['meaning']['coverage'], 0)
        self.assertIsNone(summary['axes']['lifecycle']['accuracy'])
        self.assertEqual(rows[0]['correctness'], {'meaning': True, 'lifecycle': None,
                                                  'recreatability': None})

    def test_lexical_lower_bound_numbers_and_extensions(self):
        packet = self.records[0]['packet']
        packet['shape'].update(files=1204, logical_bytes=500_000_000)
        for sentence in ('1,204 source files', 'build files', 'Rust files', 'e.g. source files',
                         '.5 GB', '...1204 files'):
            with self.subTest(sentence=sentence):
                self.assertEqual(bench.unsupported_claims(sentence, packet), [])
        for sentence in ('1204 elephants', '.5 TB', '...1205 files'):
            with self.subTest(sentence=sentence):
                self.assertTrue(bench.unsupported_claims(sentence, packet))
        words = ('one two three four five six seven eight nine ten eleven twelve thirteen '
                 'fourteen fifteen sixteen seventeen eighteen nineteen twenty hundred thousand million')
        for word in words.split():
            with self.subTest(word=word):
                self.assertTrue(bench.unsupported_claims(f'{word} files', packet))
        self.assertEqual(bench.unsupported_claims('7z files', packet), ['unsupported token: 7z'])
        packet['extensions'].append({'ext': '7z', 'files': 1, 'bytes': 1})
        self.assertEqual(bench.unsupported_claims('7z files', packet), [])
        packet['extensions'].append({'ext': 'customext', 'files': 1, 'bytes': 1})
        self.assertEqual(bench.unsupported_claims('customext files', packet), [])

    def test_issue_921_faithfulness_followups(self):
        packet = self.records[0]['packet']
        packet['shape'].update(files=612, folders=7, logical_bytes=600_000_000)
        packet['markers'] = []
        packet['extensions'] = [{'ext': 'pdf', 'files': 612, 'bytes': 600_000_000}]
        self.assertTrue(bench.unsupported_claims('Contains phantomext extensions.', packet))
        for prose in ('This folder contains files.', 'The files have extensions.',
                      'It has several files.', 'These are mostly files.'):
            with self.subTest(prose=prose):
                self.assertEqual(bench.unsupported_claims(prose, packet), [])
        self.assertTrue(bench.unsupported_claims('mostly yaml files', packet))
        self.assertTrue(bench.unsupported_claims('iso and mkv files', packet))
        self.assertEqual(bench.unsupported_claims('612 pdf files', packet), [])
        self.assertTrue(bench.unsupported_claims('613 pdf files', packet))
        self.assertEqual(bench.unsupported_claims('612 image files', packet), [])
        self.assertEqual(bench.unsupported_claims('7 pdf folders', packet), [])
        self.assertTrue(bench.unsupported_claims('8 pdf folders', packet))
        packet['shape'].update(files=1200, logical_bytes=500_000_000)
        packet['extensions'] = [{'ext': 'rlib', 'files': 1200, 'bytes': 500_000_000}]
        self.assertEqual(bench.unsupported_claims('1,200 rlib files', packet), [])
        for word in ('thirty', 'forty', 'fifty', 'sixty', 'seventy', 'eighty', 'ninety',
                     'dozen', 'half'):
            with self.subTest(word=word):
                self.assertTrue(bench.unsupported_claims(f'{word} files', packet))

    def test_marker_phrases_require_observed_marker(self):
        packet = self.records[0]['packet']
        for phrase, markers in [('git repository', ['git_dir', 'git_file_worktree']),
                                ('node modules', ['node_modules_dir']),
                                ('lightroom catalog', ['lightroom_catalog']),
                                ('lightroom catalogue', ['lightroom_catalog']),
                                ('virtual disk', ['vhdx_file']),
                                ('model weights', ['gguf_file']), ('gguf', ['gguf_file'])]:
            with self.subTest(phrase=phrase):
                packet['markers'] = []
                self.assertTrue(bench.unsupported_claims(phrase, packet))
                for marker in markers:
                    packet['markers'] = [marker]
                    self.assertEqual(bench.unsupported_claims(phrase.upper(), packet), [])

    def test_age_claims_match_named_field(self):
        packet = self.records[0]['packet']
        packet['time'] = dict.fromkeys(packet['time'])
        packet['time'].update(newest_content_write=packet['observed_at'] - 10 * 86400,
                              oldest_content_write=packet['observed_at'] - 100 * 86400,
                              git_last_commit=packet['observed_at'] - 200 * 86400)
        for sentence in ('newest write 10 days ago', 'last modified 10 days ago',
                         'oldest write 100 days ago', '200 days old'):
            self.assertEqual(bench.unsupported_claims(sentence, packet), [])
        for sentence in ('newest write 100 days ago', 'last modified 200 days ago',
                         'oldest write 10 days ago'):
            self.assertTrue(bench.unsupported_claims(sentence, packet))
        packet['time']['newest_content_write'] = None
        self.assertTrue(bench.unsupported_claims('last modified 100 days ago', packet))
        self.assertEqual(bench.unsupported_claims('100 days old', packet), [])

    def test_timeout_and_summary_format_fail_closed(self):
        self.records = self.records[:1]
        with patch.object(bench, 'call_endpoint', side_effect=TimeoutError('timed out')) as call:
            summary, rows = self.run_bench('http://localhost:9/v1', 'summarise', ['--timeout', '.01'])
        self.assertEqual(call.call_args.args[2], .01)
        self.assertEqual(summary['invalid_rate'], 1)
        self.assertIsNone(summary['unsupported_claim_rate'])
        self.assertIn('TimeoutError', rows[0]['error'])
        (self.directory / 'results' / 'summary.json').unlink()
        (self.directory / 'results' / 'records.jsonl').unlink()
        (self.directory / 'results').rmdir()
        with fake_server(['One sentence. Another sentence.']) as (endpoint, _):
            summary, rows = self.run_bench(endpoint, 'summarise')
        self.assertEqual(summary['invalid_rate'], 1)
        self.assertFalse(rows[0]['correctness'])

    def test_refuses_nonlocal_endpoints_and_git_outputs_without_network(self):
        for endpoint in ('http://example.com/v1', 'http://10.0.0.5:1234/v1',
                         'http://127.0.0.1.evil.com/v1', 'http://2130706433/v1',
                         'http://[::ffff:127.0.0.1]:1234/v1', 'http://localhost./v1',
                         'http://user@127.0.0.1:1234/v1', 'https://127.0.0.1@evil.com/v1'):
            with self.subTest(endpoint=endpoint):
                with self.assertRaisesRegex(ValueError, 'endpoint must be HTTP'):
                    bench.validate_endpoint(endpoint)
        for endpoint in ('http://127.0.0.1:1234/v1', 'http://localhost:1234/v1',
                         'http://[::1]:1234/v1'):
            self.assertEqual(bench.validate_endpoint(endpoint).port, 1234)
        for endpoint in ('http://example.com/v1', 'http://10.0.0.5:1234/v1',
                         'http://127.0.0.1@example.com/v1', 'ftp://localhost/v1',
                         'http://localhost/v1?redirect=example.com'):
            with self.subTest(endpoint=endpoint), redirect_stderr(io.StringIO()) as err:
                with self.assertRaises(SystemExit) as exit_result:
                    bench.main(['--endpoint', endpoint, '--model', 'fake',
                                '--task', 'classify', '--corpus', 'missing'])
                self.assertEqual(exit_result.exception.code, 2)
                self.assertIn('endpoint', err.getvalue())
        for marker in ('directory', 'file'):
            root = self.directory / marker
            root.mkdir()
            if marker == 'directory':
                (root / '.git').mkdir()
            else:
                (root / '.git').write_text('gitdir: unused')
            with self.assertRaisesRegex(ValueError, 'Git checkout'):
                bench.validate_output(root / 'new' / 'nested')
        with self.assertRaisesRegex(ValueError, 'Git checkout'):
            bench.validate_output(ROOT / 'bench-results')

    def test_dry_run_and_max_records(self):
        corpus = self.directory / 'corpus.json'
        corpus.write_text(json.dumps(self.records))
        with redirect_stdout(io.StringIO()) as out:
            self.assertEqual(bench.main(['--endpoint', 'http://localhost:9/v1', '--model', 'fake',
                                         '--task', 'classify', '--corpus', str(corpus), '--dry-run']), 0)
        messages = json.loads(out.getvalue())['messages']
        self.assertIn(self.records[0]['packet']['packet_id'], messages[1]['content'])
        with fake_server([json.dumps(self.labels)]) as (endpoint, requests):
            summary, _ = self.run_bench(endpoint, extra=['--max-records', '1'])
        self.assertEqual(summary['records'], 1)
        self.assertEqual(len(requests), 1)

    def test_http_error_redirect_and_bad_envelope_are_invalid(self):
        self.records = self.records[:3]
        with fake_server([(302, ''), (500, 'oops'), (200, '{}')]) as (endpoint, requests):
            summary, rows = self.run_bench(endpoint)
        self.assertEqual(len(requests), 3)
        self.assertEqual(summary['invalid_rate'], 1)
        self.assertEqual(summary['coverage'], 0)
        self.assertIsNone(summary['selective_accuracy'])
        self.assertIsNone(summary['tokens']['prompt'])
        self.assertTrue(all(row['error'] for row in rows))

    def test_answer_schema_strictness_and_percentiles(self):
        for value in ('[]', 'null', '{}', json.dumps({**self.labels, 'extra': True}),
                      json.dumps({**self.labels, 'lifecycle': 1}),
                      '{"meaning":"archive","meaning":"unknown"}'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                bench.parse_classification(value)
        self.assertEqual(bench.percentile([1, 2, 3, 4, 5], .5), 3)
        self.assertAlmostEqual(bench.percentile([1, 2, 3, 4, 5], .95), 4.8)

    def test_default_ports_are_explicit(self):
        for scheme, port, cls in [('http', 80, 'HTTPConnection'), ('https', 443, 'HTTPSConnection')]:
            with self.subTest(scheme=scheme), patch.object(bench.http.client, cls) as connection:
                connection.return_value.getresponse.return_value.status = 302
                with self.assertRaisesRegex(ValueError, 'redirects are refused'):
                    bench.call_endpoint(bench.validate_endpoint(f'{scheme}://localhost/v1'), {}, 1)
                connection.assert_called_once_with('127.0.0.1', port, timeout=1)

    def test_redirect_never_contacts_second_loopback_server(self):
        with fake_server(['unused'], host='127.0.0.2') as (target, target_requests):
            location = target.replace('/v1', '/')
            with fake_server([(302, '', {'Location': location})]) as (endpoint, requests):
                with self.assertRaisesRegex(ValueError, 'redirects are refused'):
                    bench.call_endpoint(bench.validate_endpoint(endpoint), {}, 1)
            self.assertEqual(len(requests), 1)
            self.assertEqual(len(target_requests), 0)

    def test_nested_json_is_invalid(self):
        nested = '[' * 2000 + '0' + ']' * 2000
        self.records = self.records[:2]
        with fake_server([nested, (200, nested)]) as (endpoint, _):
            summary, rows = self.run_bench(endpoint)
        self.assertEqual(summary['invalid_rate'], 1)
        self.assertTrue(all(row['error'] for row in rows))
        # Decoder nesting limits differ by Python version; pin the exception path too.
        with patch.object(bench, 'strict_json', side_effect=RecursionError('nested JSON')):
            with fake_server([nested]) as (endpoint, _):
                summary, rows = bench.run(self.records[:1], bench.validate_endpoint(endpoint),
                                          'fake', 'classify', 1, 0)
            self.assertEqual(summary['invalid_rate'], 1)
            self.assertIn('RecursionError', rows[0]['error'])
            with patch.object(bench, 'call_endpoint', return_value=(nested, {})):
                summary, rows = bench.run(self.records[:1], bench.validate_endpoint(endpoint),
                                          'fake', 'classify', 1, 0)
            self.assertEqual(summary['invalid_rate'], 1)
            self.assertIn('RecursionError', rows[0]['error'])

    def test_rows_are_flushed_before_next_record_even_on_interrupt(self):
        completed = []

        def answer(*args):
            if completed:
                rows = (self.directory / 'results' / 'records.jsonl').read_text().splitlines()
                self.assertEqual(len(rows), 1)
                self.assertEqual(json.loads(rows[0])['record_id'], self.records[0]['record_id'])
                raise KeyboardInterrupt
            completed.append(True)
            return json.dumps(self.labels), {}

        with patch.object(bench, 'call_endpoint', side_effect=answer):
            with self.assertRaises(KeyboardInterrupt):
                self.run_bench('http://localhost:9/v1')

    def test_existing_output_directory_is_refused_without_creating_files(self):
        out = self.directory / 'results'
        out.mkdir()
        (out / 'summary.json').write_text('preserve me')
        with patch.object(bench, 'call_endpoint') as call, redirect_stderr(io.StringIO()) as err:
            with self.assertRaises(SystemExit):
                self.run_bench('http://localhost:9/v1')
        call.assert_not_called()
        self.assertIn('output directory already exists', err.getvalue())
        self.assertEqual(list(out.iterdir()), [out / 'summary.json'])
        self.assertEqual((out / 'summary.json').read_text(), 'preserve me')


if __name__ == '__main__':
    unittest.main()
