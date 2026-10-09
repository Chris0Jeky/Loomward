import io
import json
import subprocess
import sys
import unittest
from pathlib import Path
from loomward.catalog import Catalog
from loomward.interop import ToolService
from loomward.mcp_stdio import Protocol, serve, MODERN, LEGACY
from test_catalog import snapshot
ROOT = Path(__file__).resolve().parents[1]

def request(method='tools/list', params=None, ident=1, modern=True):
    p = dict(params or {})
    if modern: p['_meta']={'io.modelcontextprotocol/protocolVersion':MODERN,'io.modelcontextprotocol/clientCapabilities':{}}
    return {'jsonrpc':'2.0','id':ident,'method':method,'params':p}

class McpTests(unittest.TestCase):
    def setUp(self):
        self.c = Catalog(snapshot(),disclose_names=True); self.p = Protocol(ToolService(self.c,origin='synthetic_demo'))
    def tearDown(self): self.c.close()
    def test_modern_discovery_and_stateless_tool_request(self):
        result=self.p.handle(request('server/discover'))['result']
        self.assertIn(MODERN,result['supportedVersions']);self.assertEqual(result['resultType'],'complete')
        fresh=Protocol(ToolService(self.c))
        result=fresh.handle(request('tools/call',{'name':'workspace_summary','arguments':{}}))['result']
        self.assertFalse(result['isError']);self.assertEqual(result['structuredContent']['data']['file_count'],8)
    def test_missing_metadata_and_unsupported_version(self):
        r=self.p.handle(request(modern=False));self.assertEqual(r['error']['code'],-32602)
        q=request();q['params']['_meta']['io.modelcontextprotocol/protocolVersion']='1900-01-01'
        r=self.p.handle(q);self.assertEqual(r['error']['code'],-32022)
        self.assertIn(MODERN,r['error']['data']['supported'])
    def test_capability_metadata_required_every_time(self):
        self.p.handle(request('server/discover'))
        q=request();del q['params']['_meta']['io.modelcontextprotocol/clientCapabilities']
        self.assertEqual(self.p.handle(q)['error']['code'],-32602)
    def test_legacy_initialize_is_separate(self):
        q=request('initialize',{'protocolVersion':LEGACY,'capabilities':{},'clientInfo':{'name':'test','version':'1'}},modern=False)
        r=self.p.handle(q)['result'];self.assertEqual(r['protocolVersion'],LEGACY)
        self.assertNotIn('resultType',r)
        self.assertIsNone(self.p.handle({'jsonrpc':'2.0','method':'notifications/initialized'}))
        self.assertEqual(len(self.p.handle(request(modern=False))['result']['tools']),4)
    def test_legacy_handshake_never_claims_modern_semantics(self):
        q=request('initialize',{'protocolVersion':MODERN,'capabilities':{},'clientInfo':{'name':'test','version':'1'}},modern=False)
        self.assertEqual(self.p.handle(q)['result']['protocolVersion'],LEGACY)
    def test_modern_metadata_cannot_borrow_legacy_capabilities(self):
        self.test_legacy_initialize_is_separate()
        q=request();del q['params']['_meta']['io.modelcontextprotocol/clientCapabilities']
        self.assertEqual(self.p.handle(q)['error']['code'],-32602)
    def test_tool_errors_vs_protocol_errors(self):
        r=self.p.handle(request('tools/call',{'name':'shell','arguments':{}}))
        self.assertEqual(r['error']['code'],-32602)
        r=self.p.handle(request('tools/call',{'name':'catalog_search','arguments':{'limit':0}}))['result']
        self.assertTrue(r['isError'])
    def test_unknown_method_and_bad_ids(self):
        self.assertEqual(self.p.handle(request('shell'))['error']['code'],-32601)
        for ident in (None,True,1.2,{},[]):
            self.assertEqual(self.p.handle(request(ident=ident))['error']['code'],-32600)
    def test_notifications_never_respond(self):
        for method in ('notifications/initialized','notifications/cancelled','invented'):
            self.assertIsNone(self.p.handle({'jsonrpc':'2.0','method':method,'params':{}}))
    def test_resources_and_unknown_uri(self):
        self.assertEqual(len(self.p.handle(request('resources/list'))['result']['resources']),2)
        r=self.p.handle(request('resources/read',{'uri':'file:///private'}))
        self.assertEqual(r['error']['code'],-32602)
    def test_client_self_report_does_not_grant_permission(self):
        with Catalog(snapshot()) as c:
            p=Protocol(ToolService(c));q=request('tools/call',{'name':'catalog_search','arguments':{}})
            q['params']['_meta']['io.modelcontextprotocol/clientInfo']={'name':'trusted-owner','version':'admin'}
            self.assertTrue(p.handle(q)['result']['isError'])
    def test_stdio_framing_parse_errors_and_notification_silence(self):
        messages=[b'{bad}\n',b'{"jsonrpc":"2.0","id":1,"id":2}\n',
                  json.dumps({'jsonrpc':'2.0','method':'notifications/initialized'}).encode()+b'\n',
                  json.dumps(request('ping')).encode()+b'\n']
        output=io.BytesIO();serve(self.p,io.BytesIO(b''.join(messages)),output)
        rows=[json.loads(x) for x in output.getvalue().splitlines()]
        self.assertEqual(len(rows),3);self.assertEqual(rows[0]['error']['code'],-32700)
        self.assertEqual(rows[-1]['result']['resultType'],'complete')
    def test_large_frame_is_rejected_and_stream_recovers(self):
        data=b'x'*70000+b'\n'+json.dumps(request('ping')).encode()+b'\n'
        out=io.BytesIO();serve(self.p,io.BytesIO(data),out)
        rows=[json.loads(x) for x in out.getvalue().splitlines()]
        self.assertEqual(len(rows),2);self.assertIn('error',rows[0]);self.assertIn('result',rows[1])
    def test_actual_subprocess_both_eras_clean_stdout(self):
        messages=[request('server/discover'),request('tools/call',{'name':'workspace_summary','arguments':{}},ident=2),
                  request('initialize',{'protocolVersion':LEGACY,'capabilities':{},'clientInfo':{'name':'probe','version':'1'}},ident=3,modern=False),
                  {'jsonrpc':'2.0','method':'notifications/initialized'},request(ident=4,modern=False)]
        p=subprocess.run([sys.executable,str(ROOT/'scripts/run_mcp.py'),'--demo'],input=''.join(json.dumps(x)+'\n' for x in messages),text=True,capture_output=True,timeout=10)
        self.assertEqual(p.returncode,0,p.stderr)
        rows=[json.loads(x) for x in p.stdout.splitlines()]
        self.assertEqual(len(rows),4);self.assertEqual(rows[-1]['id'],4)
        self.assertEqual(len(rows[-1]['result']['tools']),2) # name disclosure is separately opt-in
    def test_cli_requires_explicit_input_scope(self):
        p=subprocess.run([sys.executable,str(ROOT/'scripts/run_mcp.py')],text=True,capture_output=True,timeout=10)
        self.assertNotEqual(p.returncode,0);self.assertEqual(p.stdout,'')
