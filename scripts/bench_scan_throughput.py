"""One fresh-process HTTP service scan; append aggregate receipts, preserve every prior run.

Create the disposable fixture with the service's scan_throughput_fixture example first.
The seed state is copied before each synthetic run, giving both binaries the same lab identity
and an empty catalogue. Personal measurements require an explicit root and a prior owner log.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
from urllib.request import Request, urlopen


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--exe', type=Path, required=True)
    p.add_argument('--variant', choices=('main', 'branch'), required=True)
    p.add_argument('--root', type=Path, required=True)
    p.add_argument('--seed-state', type=Path)
    p.add_argument('--dataset', choices=('synthetic', 'personal'), default='synthetic')
    p.add_argument('--state', type=Path, required=True)
    p.add_argument('--receipt', type=Path, required=True)
    p.add_argument('--source', required=True, help='Git base or measured source digest')
    a = p.parse_args()
    # Only synthetic roots get an independent walk; a real root gets one product scan per variant.
    files = dirs = logical = 0
    if a.dataset == 'synthetic':
        assert a.seed_state is not None
        for directory, children, names in os.walk(a.root):
            dirs += len(children)
            files += len(names)
            logical += sum((Path(directory) / name).stat().st_size for name in names)
        assert (files, dirs, logical) == (100000, 1000, 12800000)
        shutil.copytree(a.seed_state, a.state)  # refuses reused state
    else:
        a.state.mkdir()  # refuses reused state
    log = a.state / 'scan-profile.log'
    env = {**os.environ, 'LOOMWARD_SCAN_PROFILE': '1'}
    with log.open('w', encoding='utf-8') as err:
        flags = ['--dataset', 'personal', '--allow-personal'] if a.dataset == 'personal' else []
        proc = subprocess.Popen([str(a.exe.resolve()), '--state-dir', str(a.state.resolve()),
                                 '--grant-root', str(a.root.resolve()), '--port', '0', *flags],
                                stdout=subprocess.PIPE, stderr=err, text=True, env=env)
        try:
            url = proc.stdout.readline().strip()
            assert '/#token=' in url, 'service startup failed; inspect ignored log'
            base, token = url.split('/#token=')
            seq = 0

            def call(command, payload):
                nonlocal seq
                seq += 1
                body = json.dumps({'protocol': 'loomward/3', 'request_id': f'bench_{seq}',
                                   'command': command, 'payload': payload}).encode()
                req = Request(base + '/api/v3/call', data=body, headers={
                    'Content-Type': 'application/json', 'X-Loomward-Token': token})
                with urlopen(req, timeout=30) as reply:
                    result = json.load(reply)
                assert result['ok'], 'service refused command; inspect ignored state'
                return result['result']

            roots = call('roots.list', {})['roots']
            assert len(roots) == 1
            started = time.perf_counter()
            job = call('scan.start', {'root_id': roots[0]['root_id'], 'mode': 'full'})['job']
            deadline = time.monotonic() + 1200
            while job['state'] not in ('completed', 'failed', 'cancelled'):
                assert time.monotonic() < deadline, 'scan exceeded finite benchmark budget'
                time.sleep(0.1)
                job = call('jobs.get', {'job_id': job['job_id']})['job']
            wall = time.perf_counter() - started
            progress = job['progress']
            assert job['state'] == 'completed' and not progress['limit_hit']
            if a.dataset == 'synthetic':
                assert job['coverage'] == 'complete'
                assert (progress['indexed_files'], progress['indexed_dirs'], int(progress['logical_bytes'])) == (files, dirs, logical)
                assert progress['failed'] == progress['skipped'] == 0
            else:
                files, dirs, logical = progress['indexed_files'], progress['indexed_dirs'], int(progress['logical_bytes'])
            # Confirm durable catalogue-backed root totals through the service too.
            node = call('tree.slice', {'anchor': {'kind': 'root', 'root_id': roots[0]['root_id']}, 'depth': 1, 'max_nodes': 16, 'min_share': 0.0, 'basis': 'logical', 'include_files': False})['nodes'][0]
            totals = {key: node[key] for key in ('files', 'dirs', 'logical_bytes', 'allocated_bytes')}
            assert (totals['files'], totals['dirs'], int(totals['logical_bytes'])) == (files, dirs, logical)
            totals['complete'] = node['coverage'] == 'complete'
            assert totals['complete'] == (job['coverage'] == 'complete')
        finally:
            proc.terminate()
            proc.wait(timeout=10)
    phases = {}
    for line in log.read_text(encoding='utf-8').splitlines():
        if line.startswith('scan-profile '):
            _, phase, payload = line.split(' ', 2)
            assert phase not in phases, 'expected one scan per process'
            phases[phase] = json.loads(payload)
    assert set(phases) == {'native', 'pipeline', 'writer'}
    assert phases['native']['metadata_opens']['calls'] < files, 'per-file metadata opens returned'
    assert phases['pipeline']['relist_calls'] == 0
    if a.dataset == 'personal':
        # Public real-folder evidence has rounded byte aggregates only.
        for data in (progress, totals):
            for key in ('logical_bytes', 'allocated_bytes'):
                value = data.pop(key)
                data[key.replace('_bytes', '_gib')] = None if value is None else round(int(value) / 2**30, 1)
    receipt = {'variant': a.variant, 'source': a.source, 'binary_sha256': hashlib.sha256(a.exe.read_bytes()).hexdigest(),
               'platform': 'Windows', 'host': 'shared host', 'cache': 'warm/uncontrolled',
               'root': 'synthetic service S tier' if a.dataset == 'synthetic' else 'real-B (owner-authorised, agent-picked)',
               'files': files, 'directories': dirs, 'wall_seconds': wall, 'progress': progress,
               'phases': phases, 'catalogue_totals': totals,
               'oracle': 'independent disk walk + service progress + committed root totals; exact' if a.dataset == 'synthetic' else 'service progress matches committed tree root totals; no extra real-root traversal',
               'at_utc': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}
    prior = json.loads(a.receipt.read_text()) if a.receipt.exists() else {'schema_version': 1, 'runs': []}
    prior['runs'].append(receipt)
    a.receipt.parent.mkdir(parents=True, exist_ok=True)
    a.receipt.write_text(json.dumps(prior, indent=2) + '\n')
    print(json.dumps({'variant': a.variant, 'elapsed_ms': progress['elapsed_ms'],
                      'entries_per_second': progress['entries_per_second'], 'phases': phases}))


if __name__ == '__main__':
    main()
