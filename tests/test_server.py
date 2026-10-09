import http.client
import json
import tempfile
import threading
import unittest
from pathlib import Path
from loomward.server import App, Server

class ServerTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory()
        self.app=App(Path(self.tmp.name)/'state.db',demo=True)
        self.server=Server(('127.0.0.1',0),self.app,token='test-token')
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True); self.thread.start()
    def tearDown(self):
        self.server.shutdown(); self.server.server_close(); self.thread.join(); self.tmp.cleanup()
    def request(self,method,path,body=None,headers=None,auth=True):
        h={'Content-Type':'application/json'}
        if auth: h['X-Loomward-Token']='test-token'
        h.update(headers or {})
        c=http.client.HTTPConnection('127.0.0.1',self.server.server_port,timeout=3)
        c.request(method,path,json.dumps(body) if body is not None else None,headers=h)
        r=c.getresponse(); data=r.read(); status=r.status; c.close()
        return status,json.loads(data)
    def test_state_requires_token(self):
        self.assertEqual(self.request('GET','/api/state',auth=False)[0],403)
    def test_observed_and_demo_are_explicit(self):
        status,data=self.request('GET','/api/state'); self.assertEqual(status,200); self.assertEqual(data['mode'],'demo')
        self.assertFalse(data['capabilities']['file_mutation'])
    def test_cross_origin_rejected_even_with_token(self):
        self.assertEqual(self.request('GET','/api/state',headers={'Origin':'https://evil.example'})[0],403)
    def test_rebinding_host_rejected(self):
        self.assertEqual(self.request('GET','/api/state',headers={'Host':'evil.example'})[0],403)
    def test_delete_endpoint_does_not_exist(self):
        self.assertEqual(self.request('POST','/api/delete',{'path':'x'})[0],404)
    def test_root_cannot_be_changed_via_api(self):
        self.assertEqual(self.request('POST','/api/rescan',{'root':'/'})[0],400)
    def test_duplicates_need_content_consent(self):
        self.assertEqual(self.request('POST','/api/duplicates',{})[0],400)
    def test_label_uses_current_file_and_cannot_spoof_teacher(self):
        item=self.app.inventory['files'][0]['id']
        self.assertEqual(self.request('POST','/api/label',{'item_id':item,'label':'Finance','source':'teacher'})[0],400)
        status,result=self.request('POST','/api/label',{'item_id':item,'label':'Finance','event_id':'ui-one'})
        self.assertEqual(status,200); self.assertEqual(result['event']['source'],'human'); self.assertFalse(result['filesystem_changed'])
    def test_no_arbitrary_static_file_access(self):
        self.assertEqual(self.request('GET','/../README.md')[0],404)
    def test_external_bind_rejected(self):
        with self.assertRaises(ValueError): Server(('0.0.0.0',0),self.app)
    def test_non_ascii_token_is_rejected(self):
        self.assertEqual(self.request('GET','/api/state',headers={'X-Loomward-Token':'é'})[0],403)
    def test_feedback_export_is_scoped_and_complete(self):
        item=self.app.inventory['files'][0]
        for i in range(205):
            self.app.store.feedback(self.app.scope,item['id'],{'name':'invoice.pdf'},'Finance',event_id=f'x{i}')
        self.app.store.feedback('another-profile','private',{'name':'secret.pdf'},'Finance',event_id='foreign')
        status,data=self.request('GET','/api/feedback')
        self.assertEqual(status,200);self.assertEqual(len(data['events']),205);self.assertTrue(data['complete'])
        self.assertEqual(data['mode'],'demo');self.assertNotIn('foreign',[x['event_id'] for x in data['events']])
    def test_oversized_body_is_rejected_before_processing(self):
        self.assertEqual(self.request('POST','/api/label',{'padding':'x'*65536})[0],413)
        self.assertEqual(self.app.store.events(self.app.scope),[])
    def test_wrong_media_type_is_rejected(self):
        self.assertEqual(self.request('POST','/api/label',{},headers={'Content-Type':'text/plain'})[0],415)
    def test_foreign_file_identity_cannot_be_labelled(self):
        self.assertEqual(self.request('POST','/api/label',{'item_id':'not-in-this-snapshot','label':'Finance'})[0],400)
        self.assertEqual(self.app.store.events(self.app.scope),[])
