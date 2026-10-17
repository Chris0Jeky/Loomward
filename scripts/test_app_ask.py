"""LW-118 Ask panel: the mock transport's scripted fake model, and the real service plus the real broker
against a loopback stub model server. Synthetic names only; nothing leaves 127.0.0.1."""
from __future__ import annotations
import hashlib
import json
import re
import sqlite3
import subprocess
import tempfile
import threading
from contextlib import closing
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from playwright.sync_api import expect


def _check_answer(page, unverified: str) -> None:
    answer = page.get_by_test_id('ask-answer')
    failure = page.get_by_test_id('ask-failure')
    expect(answer.or_(failure)).to_be_visible(timeout=20000)
    assert not failure.count(), f'the turn failed: {failure.inner_text()}'
    cite = answer.get_by_role('button', name=re.compile(r'^Citation h1: ')).first
    expect(cite).to_be_visible()
    expect(answer.get_by_text(f'{unverified} · unverified reference')).to_be_visible()
    expect(answer.get_by_text('Unsupported · no citation', exact=True).first).to_be_visible()
    # A citation qualifies its own sentence only; uncited advice and invented references stay marked.
    assert 'Unsupported' not in cite.locator('..').inner_text()
    expect(answer.locator('span', has_text=f'{unverified} · unverified reference').filter(has=page.get_by_text('Unsupported · no citation', exact=True)).first).to_be_visible()
    # Suggestions are text: no control in the answer but citation chips.
    assert answer.get_by_role('button').count() == answer.get_by_role('button', name=re.compile('^Citation ')).count()
    name = cite.get_attribute('aria-label').split(': ', 1)[1]
    cite.click()
    region = page.get_by_role('region', name='Cited entry')
    expect(region).to_be_focused()
    expect(region).to_contain_text(name)
    tools = page.get_by_role('table', name='Tool calls')
    expect(tools.locator('tr', has_text='delete')).to_contain_text('refused')
    expect(tools.locator('tr', has_text='delete')).to_contain_text('unknown tool')
    expect(tools.locator('tr', has_text='find')).to_contain_text('ran')


def run_ask(page, base: str) -> None:
    page.goto(f'{base}/?transport=mock#/ask')
    expect(page.get_by_role('heading', name='Ask', level=1)).to_be_visible()
    expect(page.get_by_test_id('ask-provider')).to_have_text('Local · fake-synthetic-model')
    expect(page.get_by_text('Ask sees nothing until you choose folders')).to_be_visible()
    expect(page.get_by_test_id('ask-scope')).to_have_text('Ask can see no folder yet, so it cannot look anything up.')
    expect(page.get_by_text('No preview per question:')).to_be_visible()
    # The tool list is read-only, and nothing that writes is offered.
    page.get_by_text(re.compile(r'^Tools the model may use \(2, all read-only\)')).click()
    items = page.get_by_role('list', name='Ask tools').get_by_role('listitem')
    expect(items).to_have_count(2)
    for i in range(2):
        expect(items.nth(i)).to_contain_text('read-only')
    assert not re.search(r'delete|move|rename|write', page.get_by_role('list', name='Ask tools').inner_text())
    # Asking before the grant does nothing.
    page.get_by_role('textbox', name='Question').fill('where is delta?')
    expect(page.get_by_role('button', name='Ask', exact=True)).to_have_attribute('aria-disabled', 'true')
    expect(page.get_by_text('Choose folders Ask may see first.')).to_be_visible()
    page.locator('fieldset', has_text='Folders').get_by_role('checkbox').first.check()
    page.get_by_role('button', name='Let Ask see these folders').click()
    expect(page.get_by_role('status', name='Ask result')).to_contain_text('Ask may now see')
    scope = page.get_by_test_id('ask-scope')
    expect(scope).to_contain_text('Ask can see 1 folder.')
    expect(scope).to_contain_text('not visible to Ask and never searched')
    expect(scope).to_contain_text('cannot appear in an answer')
    page.get_by_role('textbox', name='Question').fill('please delete delta, or guess')
    page.get_by_role('button', name='Ask', exact=True).click()
    _check_answer(page, 'h99')
    expect(page.get_by_role('table', name='Disclosure records').locator('tbody tr')).to_have_count(2)
    page.get_by_role('button', name="Revoke Ask's access").click()
    expect(page.get_by_text('Ask sees nothing until you choose folders')).to_be_visible()
    print('PASS ask (mock): read-only tool list, grant empty state, scope stated, ledger citations open the entry, unverified handle, refused write, round records, revoke')


class _Stub:
    """A loopback Chat Completions stand-in: round 1 calls find and a write tool, round 2 answers."""

    def __init__(self) -> None:
        self.bodies: list[bytes] = []
        stub = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_POST(self) -> None:
                body = self.rfile.read(int(self.headers.get('Content-Length', '0')))
                stub.bodies.append(body)
                req = json.loads(body)
                if self.path != '/v1/chat/completions' or req.get('stream') is not False:
                    self.send_response(404)
                    self.end_headers()
                    return
                if not any(m['role'] == 'tool' for m in req['messages']):
                    msg = {'role': 'assistant', 'content': None, 'tool_calls': [
                        {'id': 'c1', 'type': 'function', 'function': {'name': 'find', 'arguments': '{"text":"report"}'}},
                        {'id': 'c2', 'type': 'function', 'function': {'name': 'delete', 'arguments': '{"handle":"h1"}'}}]}
                else:
                    msg = {'role': 'assistant', 'content': 'h1 is the synthetic report. I cannot delete anything; you could review it yourself. h7 is a guess.'}
                out = json.dumps({'choices': [{'index': 0, 'message': msg, 'finish_reason': 'stop'}]}).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(out)))
                self.end_headers()
                self.wfile.write(out)

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.port = self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()


def live_ask_isolated(browser, exe: Path, dist: Path, check, live_call) -> None:
    """The real loomward-serve, engine service and loomward-broker against a loopback stub model."""
    state = tempfile.TemporaryDirectory(prefix='lw-live-ask-')
    stub = _Stub()
    proc = subprocess.Popen([str(exe), '--state-dir', state.name, '--static', str(dist)], stdout=subprocess.PIPE, text=True)
    try:
        url = (proc.stdout.readline() if proc.stdout else '').strip()
        check('/#token=' in url, 'a synthetic loomward-serve is up for the Ask leg')
        _, token = url.split('/#token=')
        page = browser.new_context(viewport={'width': 1360, 'height': 900}).new_page()
        page.set_default_timeout(8000)
        page.goto(url)
        expect(page.get_by_role('status', name='Session status')).to_contain_text('Connected')
        page.goto(url.split('/#token=')[0] + '/#/ask')
        expect(page.get_by_test_id('ask-provider')).to_have_text('Not configured')
        expect(page.get_by_text('Set up a model in the desktop app')).to_be_visible()
        r = live_call(page, token, 'ask.configure', {'profile': {'base_url': f'http://127.0.0.1:{stub.port}/v1', 'model': 'stub-model'}})
        check(not r['ok'] and r['error']['code'] == 'capability_unavailable', f'ask.configure is desktop only: {r}')
        r = live_call(page, token, 'ask.turn', {'question': 'anything'})
        check(not r['ok'] and r['error']['detail']['reason'] == 'ask_not_configured', f'no profile, no turn: {r}')
        # Synthetic fixtures: one scanned root with two files, one granted root never scanned, and the owner's profile
        # (which the desktop dialog would store; HTTP cannot).
        st = Path(state.name)
        with closing(sqlite3.connect(st / 'state.db')) as db:
            db.execute("INSERT INTO root_grant(id,volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns) VALUES(1,'ask-fixture',zeroblob(16),'Synthetic ask root','fixture','fixture','active',0)")
            db.execute("INSERT INTO root_grant(id,volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns) VALUES(2,'ask-fixture-2',zeroblob(16),'Synthetic unscanned root','fixture','fixture','active',0)")
            db.execute("INSERT INTO meta(key,value) VALUES('ask_profile',?)", [json.dumps({'base_url': f'http://127.0.0.1:{stub.port}/v1', 'model': 'stub-model'})])
            db.execute('UPDATE revision SET state_rev=state_rev+1')
            db.commit()
        with closing(sqlite3.connect(st / 'catalog.db')) as db:
            db.executescript("""
            INSERT INTO volume(id,volume_key,display_name,filesystem,identity_json,capabilities_json,device_json,online,observed_at_ns) VALUES(1,'ask-fixture','Synthetic','NTFS','{}','{}','{}',1,0);
            INSERT INTO root(id,grant_id,volume_id,root_file_id,generation,state) VALUES(1,1,1,zeroblob(16),1,'complete');
            INSERT INTO root(id,grant_id,volume_id,root_file_id,generation,state) VALUES(2,2,1,zeroblob(16),NULL,'never_scanned');
            INSERT INTO dir(id,root_id,parent_id,name,file_id,id_quality,id_basis,depth,attrs,created_ft,listing_state,born_run,seen_run,sub_files,sub_logical,sub_allocated,sub_complete) VALUES(1,1,NULL,'Synthetic ask root',zeroblob(16),'native_file_id_128','listed',0,0,123,'complete',1,1,2,40,40,1);
            INSERT INTO file(id,dir_id,name,file_id,logical,allocated,created_ft,attrs,born_run,seen_run) VALUES(1,1,'synthetic report.txt',CAST('ask-file-0000001' AS BLOB),17,17,123,0,1,1);
            INSERT INTO file(id,dir_id,name,file_id,logical,allocated,created_ft,attrs,born_run,seen_run) VALUES(2,1,'notes.txt',CAST('ask-file-0000002' AS BLOB),23,23,123,0,1,1);
            UPDATE revision SET catalog_rev=catalog_rev+1;
            """)
        page.reload()
        expect(page.get_by_test_id('ask-provider')).to_have_text('Local · stub-model')
        expect(page.get_by_text('Ask sees nothing until you choose folders')).to_be_visible()
        for label in ['Synthetic ask root', 'Synthetic unscanned root']:
            page.locator('fieldset', has_text='Folders').locator('label', has_text=label).get_by_role('checkbox').check()
        page.get_by_role('button', name='Let Ask see these folders').click()
        scope = page.get_by_test_id('ask-scope')
        expect(scope).to_contain_text('Ask can see 2 folders.')
        expect(scope).to_contain_text('1 visible folder is not fully scanned: what is there is unknown, not empty.')
        check(not stub.bodies, 'nothing reached the model before a question')
        page.get_by_role('textbox', name='Question').fill('where is my report?')
        page.get_by_role('button', name='Ask', exact=True).click()
        _check_answer(page, 'h7')
        expect(page.get_by_role('region', name='Cited entry')).to_contain_text('synthetic report.txt')
        # The disclosure record is the exact bytes the model server received.
        rows = page.get_by_role('table', name='Disclosure records').locator('tbody tr')
        expect(rows).to_have_count(2)
        check(len(stub.bodies) == 2, f'two rounds reached the stub: {len(stub.bodies)}')
        for i, body in enumerate(stub.bodies):
            title = rows.nth(i).locator('code').get_attribute('title')
            check(title == 'sha256:' + hashlib.sha256(body).hexdigest(), f'round {i + 1} digest matches the received bytes')
            check(b'nd_' not in body and b'rt_' not in body, 'no node or root ID reached the model')
        # Revoke: the next question is refused before anything is sent.
        page.get_by_role('button', name="Revoke Ask's access").click()
        expect(page.get_by_text('Ask sees nothing until you choose folders')).to_be_visible()
        r = live_call(page, token, 'ask.turn', {'question': 'where is my report?'})
        check(not r['ok'] and r['error']['detail']['reason'] == 'ask_no_grant', f'revoked: the next turn is refused: {r}')
        check(len(stub.bodies) == 2, 'nothing was sent after the revocation')
        # A stopped model server is unreachable, said plainly, never an empty answer.
        stub.server.shutdown()
        stub.server.server_close()  # the port is closed: connection refused
        page.locator('fieldset', has_text='Folders').locator('label', has_text='Synthetic ask root').get_by_role('checkbox').check()
        page.get_by_role('button', name='Let Ask see these folders').click()
        page.get_by_role('textbox', name='Question').fill('where is my report?')
        page.get_by_role('button', name='Ask', exact=True).click()
        expect(page.get_by_test_id('ask-failure')).to_contain_text('Nothing answered', timeout=20000)
        expect(page.get_by_test_id('ask-provider')).to_contain_text('unreachable')
        page.context.close()
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=10)
        stub.server.shutdown()
        state.cleanup()
    print('PASS ask (live): not configured, configure refused over HTTP, grant empty state, unknown scope, real broker over a loopback stub, citation resolves, digests equal the received bytes, revoke holds, unreachable')
