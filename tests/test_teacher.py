import unittest
from loomward.teacher import validate_teacher, validate_endpoint, build_request

class TeacherTests(unittest.TestCase):
    def valid(self):
        return {'item_id':'a','label':'Finance','reason':'The filename describes an invoice.','evidence':['name'], 'abstain':False}
    def test_valid_teacher_result(self):
        self.assertEqual(validate_teacher(self.valid(),'a',['Finance','Media'])['source'],'teacher')
    def test_executable_field_rejected(self):
        v=self.valid(); v['command']='delete files'
        with self.assertRaises(ValueError): validate_teacher(v,'a',['Finance'])
    def test_wrong_identity_rejected(self):
        with self.assertRaises(ValueError): validate_teacher(self.valid(),'b',['Finance'])
    def test_unknown_label_rejected(self):
        with self.assertRaises(ValueError): validate_teacher(self.valid(),'a',['Media'])
    def test_only_literal_loopback_endpoint(self):
        self.assertEqual(validate_endpoint('http://127.0.0.1:1234/v1/chat/completions'),'http://127.0.0.1:1234/v1/chat/completions')
        for u in ('https://example.com/v1/chat/completions','http://localhost:1234/v1/chat/completions','http://127.0.0.1.evil.test/v1/chat/completions','http://user:pw@127.0.0.1:1234/v1/chat/completions','file:///etc/passwd'):
            with self.assertRaises(ValueError): validate_endpoint(u)
    def test_prompt_contains_no_raw_content(self):
        p=build_request('a',{'name':'invoice.txt'},['Finance'],'local-model')
        self.assertIn('response_format',p); self.assertNotIn('tools',p)
        self.assertIn('untrusted',p['messages'][0]['content'].lower())
    def test_sensitive_feature_name_rejected(self):
        with self.assertRaises(ValueError): build_request('a',{'name':'.env'},['Finance'],'local-model')
    def test_bad_abstain_type_rejected(self):
        v=self.valid(); v['abstain']='false'
        with self.assertRaises(ValueError): validate_teacher(v,'a',['Finance'])
