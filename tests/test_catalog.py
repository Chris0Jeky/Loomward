import copy
import json
import unittest
from loomward.catalog import Catalog, QueryBudgetExceeded


def snapshot(n=8):
    return {'schema_version': 1, 'mode': 'demo', 'root': 'PRIVATE_ROOT_NOT_FOR_EXPORT',
            'files': [{'id': f'file-{i}', 'name': f'Invoice {i}.txt', 'relative_path': f'Docs/Invoice {i}.txt',
                       'size_bytes': 20 if i % 2 else 10, 'extension': '.txt', 'flags': []} for i in range(n)]}


class CatalogTests(unittest.TestCase):
    def test_scope_before_aggregation_and_sensitive_exclusion(self):
        s = snapshot(3)
        s['files'][1]['relative_path'] = 'DocsPrivate/secret.txt'
        s['files'][2]['flags'] = ['sensitive']
        with Catalog(s, prefix='Docs') as c:
            result = c.summary()
            self.assertEqual(result['file_count'], 1)
            self.assertEqual(result['logical_bytes'], '10')
            self.assertNotIn('PRIVATE_ROOT', json.dumps(result))
            self.assertNotIn('secret', json.dumps(result))
            self.assertFalse(result['live_filesystem_access'])

    def test_metadata_disclosure_default_denied(self):
        with Catalog(snapshot()) as c:
            with self.assertRaises(PermissionError): c.search()
            with self.assertRaises(PermissionError): c.explain('it_' + '0' * 64)

    def test_keyset_pages_have_no_omissions_or_duplicates(self):
        with Catalog(snapshot(23), disclose_names=True) as c:
            cursor = None; rows = []
            while True:
                page = c.search(limit=4, cursor=cursor)
                rows.extend(page['items']); cursor = page['next_cursor']
                if cursor is None: break
            self.assertEqual(len(rows), 23)
            self.assertEqual(len({r['item_ref'] for r in rows}), 23)
            self.assertEqual([int(r['size_bytes']) for r in rows], sorted((int(r['size_bytes']) for r in rows), reverse=True))

    def test_cursor_cannot_change_query_or_scope(self):
        with Catalog(snapshot(), disclose_names=True) as a, Catalog(snapshot(), prefix='Docs', disclose_names=True) as b:
            cur = a.search(limit=2)['next_cursor']
            with self.assertRaises(ValueError): a.search(query='Invoice', cursor=cur)
            with self.assertRaises(ValueError): a.search(extension='.txt', cursor=cur)
            with self.assertRaises(ValueError): b.search(cursor=cur)
            with self.assertRaises(ValueError): a.search(cursor=cur[:-5] + 'aaaaa')

    def test_off_scope_handles_never_resolve(self):
        with Catalog(snapshot(), disclose_names=True) as a, Catalog(snapshot(), prefix='Other', disclose_names=True) as b:
            ref = a.search(limit=1)['items'][0]['item_ref']
            with self.assertRaises(LookupError): b.explain(ref)

    def test_explain_identifies_evidence_limit(self):
        with Catalog(snapshot(), disclose_names=True) as c:
            ref = c.search(limit=1)['items'][0]['item_ref']
            result = c.explain(ref)
            self.assertEqual(result['item']['item_ref'], ref)
            self.assertFalse(result['execution_authority'])
            self.assertEqual(result['freshness'], 'unverified_snapshot')
            self.assertNotIn('PRIVATE_ROOT', json.dumps(result))

    def test_rejects_unsafe_or_ambiguous_paths(self):
        for path in ('../x', '/root/x', r'C:\\x', r'\\host\share', 'Docs/../x', 'Docs//x', 'Docs/./x', 'a\x00b', 'Docs/file:stream'):
            s = snapshot(1); s['files'][0]['relative_path'] = path
            with self.assertRaises(ValueError, msg=path): Catalog(s, disclose_names=True)
        for prefix in ('../', '/Docs', 'Docs/../x'):
            with self.assertRaises(ValueError): Catalog(snapshot(), prefix=prefix)

    def test_separator_equivalence_not_string_prefix(self):
        s = snapshot(2); s['files'][0]['relative_path'] = r'Docs\Sub\one.txt'
        s['files'][1]['relative_path'] = 'Docs/Submarine/two.txt'
        with Catalog(s, prefix=r'Docs\Sub', disclose_names=True) as c:
            self.assertEqual(c.summary()['file_count'], 1)

    def test_invalid_records_and_duplicate_identity(self):
        for value in (True, -1, 1.25, 2**63):
            s = snapshot(1); s['files'][0]['size_bytes'] = value
            with self.assertRaises(ValueError): Catalog(s)
        s = snapshot(1); s['files'].append(copy.deepcopy(s['files'][0]))
        with self.assertRaises(ValueError): Catalog(s)
        with self.assertRaises(ValueError): Catalog({'schema_version': 8, 'files': []})

    def test_literal_search_not_sql_or_wildcard_language(self):
        s = snapshot(3); s['files'][0]['name'] = '100%_done.txt'; s['files'][0]['relative_path'] = 'Docs/100%_done.txt'
        with Catalog(s, disclose_names=True) as c:
            self.assertEqual(len(c.search(query='%_')['items']), 1)
            self.assertEqual(len(c.search(query="' OR 1=1 --")['items']), 0)
            self.assertEqual(len(c.search(query='invoice', extension='.txt')['items']), 2)

    def test_query_budget_is_explicit_failure_not_empty_success(self):
        with Catalog(snapshot(1000), disclose_names=True, query_step_budget=1000) as c:
            with self.assertRaises(QueryBudgetExceeded): c.search(query='not present')
            self.assertEqual(len(c.search(limit=1)['items']), 1)

    def test_bounded_query_and_output(self):
        with Catalog(snapshot(), disclose_names=True) as c:
            for value in (True, 0, 101, 2.5):
                with self.assertRaises(ValueError): c.search(limit=value)
            with self.assertRaises(ValueError): c.search(query='x' * 129)
            with self.assertRaises(ValueError): c.search(cursor='x' * 3000)

    def test_large_exact_byte_counts_are_strings(self):
        s = snapshot(1); s['files'][0]['size_bytes'] = 2**53 + 1
        with Catalog(s, disclose_names=True) as c:
            self.assertEqual(c.summary()['logical_bytes'], str(2**53 + 1))
            self.assertEqual(c.search()['items'][0]['size_bytes'], str(2**53 + 1))

    def test_construction_does_not_change_snapshot(self):
        s = snapshot(); expected = copy.deepcopy(s)
        with Catalog(s): pass
        self.assertEqual(s, expected)

    def test_query_plan_uses_compound_keyset_index(self):
        with Catalog(snapshot(100), disclose_names=True) as c:
            self.assertTrue(any('catalog_order' in line for line in c.query_plan()))
            self.assertFalse(any('TEMP B-TREE' in line for line in c.query_plan()))

    def test_sensitive_record_cannot_be_revealed_by_search(self):
        s = snapshot(2); s['files'][0]['flags'] = ['sensitive']; s['files'][0]['name'] = 'secret.txt'
        with Catalog(s, disclose_names=True) as c:
            self.assertEqual(c.search(query='secret')['items'], [])
            self.assertEqual(c.summary()['file_count'], 1)

class CatalogBoundsRegressionTests(unittest.TestCase):
    def test_unicode_flags_cannot_create_an_unpageable_first_row(self):
        s=snapshot(3)
        for f in s['files']:
            f.update(name='🌠'*4096,relative_path='Docs/'+'🌠'*2000,flags=['🌠'*128]*32)
        with Catalog(s,disclose_names=True) as c:
            result=c.search(limit=2)
            self.assertGreater(len(result['items']),0)
            self.assertTrue(result['items'][0]['display_truncated'])
            self.assertLess(len(json.dumps(result).encode()),56000)
            if result['has_more']: self.assertIsNotNone(result['next_cursor'])

class CatalogFingerprintPrivacyTests(unittest.TestCase):
    def test_generation_is_view_keyed_not_a_public_metadata_fingerprint(self):
        with Catalog(snapshot()) as a, Catalog(snapshot()) as b:
            self.assertNotEqual(a.summary()['snapshot_generation'], b.summary()['snapshot_generation'])
    def test_hidden_records_do_not_change_same_key_view_generation(self):
        from unittest.mock import patch
        s=snapshot(1);other=copy.deepcopy(s)
        other['files'].append({'id':'hidden','name':'secret','relative_path':'Private/secret','size_bytes':100,'extension':'','flags':[]})
        with patch('loomward.catalog.secrets.token_bytes',return_value=b'a'*32):
            with Catalog(s,prefix='Docs') as a, Catalog(other,prefix='Docs') as b:
                self.assertEqual(a.summary(),b.summary())
