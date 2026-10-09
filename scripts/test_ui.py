"""Browser integration checks. Requires Playwright and a Chromium installation."""
from __future__ import annotations
import argparse
import http.client
from urllib.parse import urlsplit
import json
import sys
import tempfile
import threading
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'python'))
from loomward.server import App, Server
from playwright.sync_api import sync_playwright


def main():
    p=argparse.ArgumentParser();p.add_argument('--browser',default=None);p.add_argument('--screenshots',type=Path);a=p.parse_args()
    with tempfile.TemporaryDirectory() as temp:
        app=App(Path(temp)/'state.db',demo=True)
        server=Server(('127.0.0.1',0),app,token='browser-test-token')
        t=threading.Thread(target=server.serve_forever,daemon=True);t.start()
        try:
            with sync_playwright() as pw:
                browser=pw.chromium.launch(executable_path=a.browser,headless=True,args=['--no-sandbox'])
                page=browser.new_page(viewport={'width':1480,'height':1060},device_scale_factor=1)
                page.set_default_timeout(7000)
                def screenshot(name):
                    page.evaluate("window.scrollTo(0,0);document.activeElement?.blur()")
                    page.wait_for_timeout(3300)
                    page.screenshot(path=str(a.screenshots/name),full_page=True)
                errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
                # Managed Chromium forbids URL navigation. Render assets with set_content;
                # bridge fetch to the real HTTP server, preserving API status/JSON semantics.
                def bridge(path,options):
                    conn=http.client.HTTPConnection('127.0.0.1',server.server_port,timeout=10)
                    conn.request(options.get('method','GET'),path,body=options.get('body'),headers=options.get('headers',{}))
                    response=conn.getresponse();body=response.read().decode();status=response.status
                    headers=dict(response.getheaders());conn.close()
                    return {'status':status,'body':body,'headers':headers}
                page.expose_function('__httpBridge',bridge)
                root=Path(__file__).resolve().parents[1]/'ui'
                document=(root/'index.html').read_text()
                document=document.replace('<link rel="stylesheet" href="/styles.css">','<style>'+(root/'styles.css').read_text()+'</style>')
                document=document.replace('<script src="/demo-data.js" defer></script><script src="/expansion.js" defer></script><script src="/app.js" defer></script>','')
                hook="<script>if(!crypto.randomUUID)crypto.randomUUID=()=>[...crypto.getRandomValues(new Uint8Array(16))].map(x=>x.toString(16).padStart(2,'0')).join('');window.fetch=async(path,options)=>{const r=await window.__httpBridge(path,options||{});return new Response(r.body,{status:r.status,headers:r.headers})};history.replaceState=()=>{location.hash=''};</script>"
                document=document.replace('</body>',hook+'<script>'+(root/'demo-data.js').read_text()+'</script><script>'+(root/'expansion.js').read_text()+'</script><script>'+(root/'app.js').read_text()+'</script></body>')
                page.evaluate("location.hash='token=browser-test-token'")
                page.set_content(document)
                page.get_by_role('heading',name='Your workspace, in balance.').wait_for()
                assert 'Python reference' in page.locator('#footer-status').inner_text();print('PASS live connection')
                page.locator('.skip-link').evaluate('(el)=>el.click()')
                assert 'token=' in page.url
                page.set_content(document);page.get_by_role('heading',name='Your workspace, in balance.').wait_for()
                assert 'Python reference' in page.locator('#footer-status').inner_text(),'Refresh silently lost the live session'
                print('PASS refresh preserves runtime')
                if a.screenshots:
                    a.screenshots.mkdir(parents=True,exist_ok=True)
                    screenshot('overview.png')
                page.get_by_role('button',name='Organise & learn',exact=True).click()
                before=app.model.training_count
                page.locator('.label-choice').first.select_option('Finance')
                page.get_by_role('button',name='Save label',exact=True).first.click()
                page.wait_for_function("document.querySelector('#toast').textContent.includes('Student retrained')")
                assert app.model.training_count==before+1;print('PASS persistent feedback and retraining')
                page.get_by_role('button',name='Disk tiers',exact=True).click()
                page.locator('[data-action="simulate"]').click()
                page.wait_for_function("document.querySelector('#plan-results').textContent.includes('GiB')")
                assert app.last_plan and not any(x['executable'] for x in app.last_plan['proposals']);print('PASS non-executable tier plan')
                if a.screenshots:screenshot('disk-tiers.png')
                page.get_by_role('button',name='Storage explorer',exact=True).click()
                page.locator('#file-search').fill('invoice')
                assert page.locator('#file-table tbody tr').count()>=1;print('PASS search')
                page.locator('[data-action="duplicates"]').first.click()
                page.wait_for_function("document.querySelector('#content').textContent.includes('byte_equal') || document.querySelector('#content').textContent.includes('Duplicate')")
                assert app.duplicates is not None;print('PASS duplicate review')
                for nav in ['Processes','Activity','Protection']:
                    page.get_by_role('button',name=nav,exact=True).click()
                    assert page.locator('h1').count()==1
                print('PASS all navigation pages')
                page.get_by_role('button',name='Decision desk',exact=True).click()
                page.get_by_role('heading',name='Decisions, with the evidence attached.').wait_for()
                page.locator('[data-action="x-decision"][data-choice="hold"]').click()
                assert 'Held for review' in page.locator('#decision-outcome').inner_text()
                assert 'No authority granted' in page.locator('#content').inner_text()
                print('PASS decision choices are review notes, not approval')
                if a.screenshots:screenshot('decision-desk.png')
                page.get_by_role('button',name='Connections',exact=True).click()
                assert page.locator('[data-action="x-provider"]').count()>=4
                page.locator('[data-action="x-provider"][data-provider="estate"]').click()
                assert 'Not connected' in page.locator('#provider-detail').inner_text()
                page.locator('#mcp-tool').select_option('catalog_search')
                page.locator('[data-action="x-build-request"]').click()
                assert 'Disclosure not enabled' in page.locator('#mcp-request').inner_text()
                page.locator('#mcp-disclose').check()
                page.locator('[data-action="x-build-request"]').click()
                assert '2026-07-28' in page.locator('#mcp-request').inner_text()
                assert 'catalog_search' in page.locator('#mcp-request').inner_text()
                assert 'No request was sent' in page.locator('#content').inner_text()
                print('PASS MCP request builder requires disclosure and sends nothing')
                if a.screenshots:screenshot('connections.png')
                page.get_by_role('button',name='Resource budgets',exact=True).click()
                page.locator('#budget-memory').fill('1024')
                page.locator('[data-action="x-admit"]').click()
                page.wait_for_function("document.querySelector('#admission-results').textContent.includes('insufficient_memory_mib')")
                assert 'Python reference' in page.locator('#admission-results').inner_text()
                assert '0 processes changed' in page.locator('#admission-results').inner_text()
                print('PASS live vector admission simulation and visible refusal')
                if a.screenshots:screenshot('resource-budgets.png')

                bad=json.loads(json.dumps(app.inventory));bad['coverage']['skipped_count']='<img src=x onerror="window.injected=true">'
                page.locator('#snapshot-file').set_input_files({'name':'bad.json','mimeType':'application/json','buffer':json.dumps(bad).encode()})
                page.wait_for_timeout(200)
                assert page.locator('#workspace-kind').inner_text()=='Demo workspace','Malformed snapshot accepted'
                assert page.evaluate('window.injected') is None;print('PASS hostile import rejected')
                valid=json.loads(json.dumps(app.inventory));valid['files'][0]['name']='<img src=x onerror="window.injected=true">'
                page.locator('#snapshot-file').set_input_files({'name':'safe.json','mimeType':'application/json','buffer':json.dumps(valid).encode()})
                page.wait_for_function("document.querySelector('#workspace-kind').textContent==='Imported snapshot'")
                assert page.evaluate('window.injected') is None
                assert page.locator('#file-table img').count()==0;print('PASS file names rendered as text')
                page.set_viewport_size({'width':390,'height':844})
                page.get_by_role('button',name='Overview',exact=True).click()
                assert page.evaluate('document.documentElement.scrollWidth <= innerWidth+1');print('PASS mobile viewport width')
                if a.screenshots:screenshot('mobile.png')
                assert not errors,errors;print('PASS no browser exceptions')
                browser.close()
        finally:server.shutdown();server.server_close();t.join()
if __name__=='__main__':main()
