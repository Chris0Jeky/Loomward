"""Run available verification gates; missing native tools are never called a pass.

Flags:
  --require-native  fail the run when cargo is missing
  --ui              also run the Chromium bridged-UI integration (scripts/test_ui.py)
  --browser PATH    Chromium executable for --ui
  --app             also run the Svelte app in app/: check, unit tests, build, then the
                    Playwright e2e (scripts/test_app.py), stopping at the first failure;
                    reports UNVERIFIED when Node/npm or app/node_modules is missing (#154)
"""
import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def main():
    p=argparse.ArgumentParser();p.add_argument('--require-native',action='store_true');p.add_argument('--ui',action='store_true');p.add_argument('--browser');p.add_argument('--app',action='store_true',help='also run the Svelte app checks, build and Playwright e2e (app/)');a=p.parse_args()
    env=dict(os.environ,PYTHONPATH=str(ROOT/'python'));failed=False
    commands=[('Python suite',[sys.executable,'-m','unittest','discover','-s','tests'])]
    if shutil.which('node'):
        commands.extend([('JavaScript app syntax',['node','--check','ui/app.js']),
                         ('JavaScript expansion syntax',['node','--check','ui/expansion.js']),
                         ('JavaScript boundary and Python parity fixtures',['node','tests/test_ui_logic.mjs'])])
    else:print('SKIP JavaScript syntax: Node is missing',flush=True)
    if shutil.which('cargo'):
        commands.extend([('Rust format',['cargo','fmt','--all','--','--check']),('Rust unit tests',['cargo','test','--workspace']),('Rust lint',['cargo','clippy','--workspace','--all-targets','--','-D','warnings'])])
    else:
        print('UNVERIFIED Rust: cargo is missing; no native success is claimed',flush=True)
        failed=a.require_native
    if a.ui:
        c=[sys.executable,'scripts/test_ui.py'];c+=['--browser',a.browser] if a.browser else [];commands.append(('Chromium bridged-UI integration',c))
    app=[]
    if a.app:
        npm=shutil.which('npm.cmd' if os.name=='nt' else 'npm')
        if not npm or not shutil.which('node'):print('UNVERIFIED Svelte app: Node or npm is missing; no app success is claimed',flush=True)
        elif not (ROOT/'app'/'node_modules').is_dir():print('UNVERIFIED Svelte app: app/node_modules is missing (run `npm ci --prefix app`); no app success is claimed',flush=True)
        else:app=[('Svelte app check',[npm,'--prefix','app','run','check']),('Svelte app unit tests',[npm,'--prefix','app','run','test']),('Svelte app build',[npm,'--prefix','app','run','build']),('Svelte app Playwright e2e',[sys.executable,'scripts/test_app.py'])]
    for name,command in commands:
        print('\nRUN '+name,flush=True)
        result=subprocess.run(command,cwd=ROOT,env=env,check=False)
        if result.returncode:failed=True;print('FAIL '+name,flush=True)
        else:print('PASS '+name,flush=True)
    for name,command in app:
        print('\nRUN '+name,flush=True)
        result=subprocess.run(command,cwd=ROOT,env=env,check=False)
        if result.returncode:
            failed=True;print('FAIL '+name,flush=True);print('STOP Svelte app legs after the first failure',flush=True);break
        print('PASS '+name,flush=True)
    return 1 if failed else 0
if __name__=='__main__':raise SystemExit(main())
