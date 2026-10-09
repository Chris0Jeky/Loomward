import json
import threading
import unittest
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from loomward.teacher import request_teacher,validate_endpoint
class Handler(BaseHTTPRequestHandler):
    def log_message(self,*args):pass
    def do_POST(self):
        n=int(self.headers['Content-Length']);self.server.requests.append(json.loads(self.rfile.read(n)))
        self.send_response(self.server.status)
        if self.server.status==302:self.send_header('Location','http://example.invalid/escape')
        raw=self.server.response
        self.send_header('Content-Length',str(len(raw)));self.end_headers();self.wfile.write(raw)
class TeacherHTTPTests(unittest.TestCase):
    def setUp(self):
        self.server=ThreadingHTTPServer(('127.0.0.1',0),Handler);self.server.requests=[];self.server.status=200
        self.decision={'item_id':'item-1','label':'Finance','reason':'Invoice name','evidence':['name'],'abstain':False}
        self.set_response(self.decision)
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
        self.endpoint=f'http://127.0.0.1:{self.server.server_port}/v1/chat/completions'
    def tearDown(self):self.server.shutdown();self.server.server_close();self.thread.join()
    def set_response(self,decision):self.server.response=json.dumps({'choices':[{'message':{'content':json.dumps(decision)}}]}).encode()
    def call(self,**kwargs):return request_teacher(self.endpoint,'test-model','item-1',{'name':'invoice.pdf'},['Finance','Documents'],**kwargs)
    def test_real_loopback_roundtrip_is_weak_advice(self):
        result=self.call(consent_metadata=True);self.assertEqual(result['source'],'teacher');self.assertFalse(result['autonomy_allowed'])
        request=self.server.requests[0];self.assertEqual(request['model'],'test-model');self.assertEqual(request['max_tokens'],512);self.assertNotIn('tools',request)
    def test_no_consent_makes_no_request(self):
        with self.assertRaises(ValueError):self.call()
        self.assertEqual(self.server.requests,[])
    def test_redirect_does_not_follow_to_network(self):
        self.server.status=302
        with self.assertRaises(ValueError):self.call(consent_metadata=True)
        self.assertEqual(len(self.server.requests),1)
    def test_wrong_item_id_is_rejected_after_transport(self):
        self.decision['item_id']='other';self.set_response(self.decision)
        with self.assertRaises(ValueError):self.call(consent_metadata=True)
    def test_executable_extra_field_is_rejected(self):
        self.decision['command']='remove-file';self.set_response(self.decision)
        with self.assertRaises(ValueError):self.call(consent_metadata=True)
    def test_oversized_response_is_rejected(self):
        self.server.response=b'x'*(1024**2+1)
        with self.assertRaises(ValueError):self.call(consent_metadata=True)
    def test_duplicate_response_key_is_rejected(self):
        self.server.response=b'{"choices":[],"choices":[]}'
        with self.assertRaises(ValueError):self.call(consent_metadata=True)
    def test_hostname_cannot_replace_literal_loopback(self):
        for endpoint in ['http://localhost:1234/v1/chat/completions','http://127.0.0.1.example:1234/v1/chat/completions','http://127.0.0.1:1234/v1/chat/completions?target=remote']:
            with self.assertRaises(ValueError):validate_endpoint(endpoint)
