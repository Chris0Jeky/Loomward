"""Print or explicitly execute creation of a NEW GitHub repository. Defaults private.
Requires a clean, committed local checkout and an authenticated GitHub CLI.
Never reads credentials or overwrites an existing remote repository.
"""
import argparse
import json
import re
import shutil
import subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def call(args):return subprocess.run(args,cwd=ROOT,text=True,capture_output=True,check=True,timeout=60).stdout.strip()
def main():
    p=argparse.ArgumentParser();p.add_argument('--owner',required=True);p.add_argument('--repo',default='loomward');p.add_argument('--execute',action='store_true');p.add_argument('--public',action='store_true');p.add_argument('--confirm-public');a=p.parse_args()
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9-]{0,38}',a.owner) or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]{0,99}',a.repo):raise SystemExit('Invalid owner or repository name')
    full=a.owner+'/'+a.repo
    command=['gh','repo','create',full,'--public' if a.public else '--private','--source','.', '--remote','origin','--push','--description','Local-first learned workspace, storage and resource companion. Observation-only research prototype.']
    print(json.dumps({'dry_run':not a.execute,'command':command,'repository':full},indent=2))
    if not a.execute:return 0
    if a.public and a.confirm_public!=full:raise SystemExit('Public publication requires --confirm-public OWNER/REPO and a review of tracked files')
    if not shutil.which('gh') or not shutil.which('git'):raise SystemExit('GitHub CLI and Git are required; no changes made')
    if Path(call(['git','rev-parse','--show-toplevel'])).resolve()!=ROOT:raise SystemExit('Run from a standalone committed repository, not a parent repository')
    if call(['git','status','--porcelain']):raise SystemExit('Commit or reconcile local changes before publishing')
    call(['git','rev-parse','--verify','HEAD'])
    if 'origin' in call(['git','remote']).splitlines():raise SystemExit('An origin remote already exists. Inspect it manually; this helper will not replace it')
    files=call(['git','ls-files']).splitlines()
    bad=[x for x in files if any(part in ('.venv','node_modules','target','.loomward','__pycache__') for part in Path(x).parts) or x.lower().endswith(('.db','.sqlite','.sqlite3','.pem','.pfx','.key')) or Path(x).name=='.env']
    if bad:raise SystemExit('Potential private/runtime files tracked: '+', '.join(bad[:10]))
    if call(['gh','api','user','--jq','.login']).lower()!=a.owner.lower():raise SystemExit('This helper supports creation only in the authenticated personal account')
    probe=subprocess.run(['gh','repo','view',full,'--json','nameWithOwner'],cwd=ROOT,capture_output=True,text=True,timeout=60)
    if probe.returncode==0:raise SystemExit('Repository already exists; refusing to push into it')
    subprocess.run(command,cwd=ROOT,check=True,timeout=180)
    print('Created and pushed the repository. Issues are a separate explicit step.');return 0
if __name__=='__main__':raise SystemExit(main())
