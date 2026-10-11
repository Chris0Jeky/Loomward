"""One fresh-process HTTP service scan; append aggregate receipts, preserve every prior run.

The #148 KPI (owner decision q-13) is end-to-end files/s through the service on lab and real
trees; 250k rows/s stays a stretch note. Each receipt carries a `kpi` block (files/s by engine
elapsed and by HTTP wall, plus the main phase seconds) and the host's sampled CPU load, labelled
"quiet host" below 25 percent. `--summarize RECEIPT` prints per-variant medians and ranges.

Create the disposable fixture with the service's scan_throughput_fixture example first.
The seed state is copied before each synthetic run, giving both binaries the same lab identity
and an empty catalogue. Personal measurements require an explicit root, --roots-log (the root is
appended and flushed before the service starts) and an anonymised --label for the receipt.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import time
from urllib.request import Request, urlopen

QUIET_CPU_PERCENT = 25.0
STRETCH_ROWS_PER_SECOND = 250_000


def host_cpu(seconds):
    """Mean total CPU percent over `seconds` (Windows performance counter), or None."""
    if seconds <= 0:
        return None
    samples = max(1, seconds // 2)
    command = (r"(Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 2 "
               f"-MaxSamples {samples}).CounterSamples.CookedValue | Measure-Object -Average "
               "| ForEach-Object Average")
    try:
        out = subprocess.run(['powershell', '-NoProfile', '-Command', command], capture_output=True,
                             text=True, timeout=seconds + 60).stdout.strip()
        return round(float(out), 1)
    except (OSError, ValueError, subprocess.TimeoutExpired):
        return None


def kpi(files, wall, progress, phases):
    writer, pipeline = phases['writer'], phases['pipeline']
    return {
        'files_per_second': files / (progress['elapsed_ms'] / 1000),
        'files_per_second_http_wall': files / wall,
        'stretch_rows_per_second': STRETCH_ROWS_PER_SECOND,
        'phase_seconds': {
            'scan_elapsed': progress['elapsed_ms'] / 1000,
            'pipeline_persistence': pipeline['persistence_seconds'],
            'pipeline_aggregation_and_end': pipeline['aggregation_seconds'],
            'native_enumeration_queries': phases['native']['enumeration_queries']['seconds'],
            'writer_staging': writer['staging_seconds'],
            'writer_listing_publication': writer['listing_seconds'],
            'writer_file_insert': writer['file_insert_seconds'],
            'writer_commit': writer['commit_seconds'],
            'writer_multilink': writer['multilink_seconds'],
            'writer_rollup': writer['rollup_seconds'],
            'writer_checkpoint': writer['checkpoint_seconds'],
        },
        'transactions': writer['transactions'],
    }


def summarize(path, first):
    runs = json.loads(path.read_text())['runs'][first:]
    for variant in ('main', 'branch'):
        chosen = [r for r in runs if r['variant'] == variant and 'kpi' in r]
        if not chosen:
            continue
        rows = {'files_per_second': [r['kpi']['files_per_second'] for r in chosen],
                'files_per_second_http_wall': [r['kpi']['files_per_second_http_wall'] for r in chosen]}
        for phase in chosen[0]['kpi']['phase_seconds']:
            rows[phase] = [r['kpi']['phase_seconds'][phase] for r in chosen]
        print(f"{variant}: {len(chosen)} runs; hosts {sorted({r['host'] for r in chosen})}")
        for key, values in rows.items():
            print(f"  {key}: median {statistics.median(values):.3f} [{min(values):.3f}-{max(values):.3f}]")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--summarize', type=Path, help='print medians and ranges of an existing receipt file, then exit')
    p.add_argument('--first-index', type=int, default=0, help='with --summarize: the first run index to include')
    p.add_argument('--host-sample-seconds', type=int, default=30,
                   help='CPU sampling before the scan; 0 skips it and labels the run "shared host"')
    p.add_argument('--max-cpu', type=float, help='refuse to measure when the sampled CPU percent exceeds this')
    p.add_argument('--exe', type=Path)
    p.add_argument('--variant', choices=('main', 'branch'))
    p.add_argument('--root', type=Path)
    p.add_argument('--seed-state', type=Path)
    p.add_argument('--dataset', choices=('synthetic', 'personal'), default='synthetic')
    p.add_argument('--state', type=Path)
    p.add_argument('--receipt', type=Path)
    p.add_argument('--source', help='Git base or measured source digest')
    p.add_argument('--roots-log', type=Path, help='personal only: the gitignored .loomward/scan-roots.log; the root is appended before scanning')
    p.add_argument('--label', help='personal only: the anonymised label written to the receipt, e.g. "real-B (owner-authorised, agent-picked)"')
    a = p.parse_args()
    if a.summarize:
        return summarize(a.summarize, a.first_index)
    missing = [name for name in ('exe', 'variant', 'root', 'state', 'receipt', 'source') if getattr(a, name) is None]
    if missing:
        p.error('required: ' + ', '.join('--' + name for name in missing))
    cpu = host_cpu(a.host_sample_seconds)
    if a.max_cpu is not None and (cpu is None or cpu > a.max_cpu):
        p.error(f'host CPU {cpu} percent exceeds --max-cpu {a.max_cpu}; not measuring')
    if a.dataset == 'personal':
        # Owner grant q-4: every agent-picked real root is recorded before it is scanned.
        if a.roots_log is None or a.label is None:
            p.error('--dataset personal requires --roots-log and --label')
        if a.roots_log.parent.name != '.loomward':
            p.error('--roots-log must be a file in a gitignored .loomward/ directory')
        with a.roots_log.open('a', encoding='utf-8') as roots_log:
            roots_log.write(f"{time.strftime('%Y-%m-%dT%H:%M:%S%z')}\tbench_scan_throughput\t{a.root.resolve()}\n")
            roots_log.flush()
            os.fsync(roots_log.fileno())
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
    assert set(phases) in ({'native', 'pipeline', 'writer'}, {'native', 'pipeline', 'writer', 'checkpoint'})
    assert phases['native']['metadata_opens']['calls'] < files, 'per-file metadata opens returned'
    assert phases['pipeline']['relist_calls'] == 0
    if a.dataset == 'personal':
        # Public real-folder evidence has rounded byte aggregates only.
        for data in (progress, totals):
            for key in ('logical_bytes', 'allocated_bytes'):
                value = data.pop(key)
                data[key.replace('_bytes', '_gib')] = None if value is None else round(int(value) / 2**30, 1)
    receipt = {'variant': a.variant, 'source': a.source, 'binary_sha256': hashlib.sha256(a.exe.read_bytes()).hexdigest(),
               'platform': 'Windows', 'cache': 'warm/uncontrolled',
               'host': 'quiet host' if cpu is not None and cpu < QUIET_CPU_PERCENT else 'shared host',
               'host_cpu_percent_before': cpu, 'kpi': kpi(files, wall, progress, phases),
               'root': 'synthetic service S tier' if a.dataset == 'synthetic' else a.label,
               'files': files, 'directories': dirs, 'wall_seconds': wall, 'progress': progress,
               'phases': phases, 'catalogue_totals': totals,
               'oracle': 'independent disk walk + service progress + committed root totals; exact' if a.dataset == 'synthetic' else 'service progress matches committed tree root totals; no extra real-root traversal',
               'at_utc': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}
    prior = json.loads(a.receipt.read_text()) if a.receipt.exists() else {'schema_version': 1, 'runs': []}
    prior['runs'].append(receipt)
    a.receipt.parent.mkdir(parents=True, exist_ok=True)
    a.receipt.write_text(json.dumps(prior, indent=2) + '\n')
    print(json.dumps({'variant': a.variant, 'elapsed_ms': progress['elapsed_ms'],
                      'entries_per_second': progress['entries_per_second'], 'host': receipt['host'],
                      'host_cpu_percent_before': cpu, 'kpi': receipt['kpi'], 'phases': phases}))


if __name__ == '__main__':
    main()
