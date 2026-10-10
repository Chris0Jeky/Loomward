"""Run available verification gates; missing native tools are never called a pass.

Flags:
  --require-native  fail the run when cargo is missing
  --ui              also run the Chromium bridged-UI integration (scripts/test_ui.py)
  --browser PATH    Chromium executable for --ui
  --app             also run the Svelte app in app/: check, unit tests, build, then the
                    Playwright e2e (scripts/test_app.py), stopping at the first failure;
                    reports UNVERIFIED when Node/npm or app/node_modules is missing, and
                    UNVERIFIED for only the e2e when the Playwright package or its Chromium
                    is missing (#154, #186)
"""
import argparse
import importlib.util
import os
import shutil
import subprocess
import sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def playwright_gap():
    """Why the Playwright e2e cannot run, or None when the package and its bundled Chromium are present."""
    if importlib.util.find_spec('playwright') is None:return 'the Python playwright package is not installed (py -3 -m pip install playwright)'
    probe=subprocess.run([sys.executable,'-c','import sys\nfrom playwright.sync_api import sync_playwright\nwith sync_playwright() as p:sys.stdout.write(p.chromium.executable_path)'],capture_output=True,text=True,check=False)
    if probe.returncode:return 'Playwright could not start (py -3 -m playwright install chromium)'
    if not os.path.exists(probe.stdout.strip()):return 'the Chromium browser for Playwright is not installed (py -3 -m playwright install chromium)'
    return None
def app_legs(root,which,isdir,e2e_gap):
    """Return (legs, notes): legs run in order; notes are UNVERIFIED lines printed instead of skipped legs."""
    npm=which('npm.cmd' if os.name=='nt' else 'npm')
    if not npm or not which('node'):return [],['UNVERIFIED Svelte app: Node or npm is missing; no app success is claimed']
    if not isdir(root/'app'/'node_modules'):return [],['UNVERIFIED Svelte app: app/node_modules is missing (run `npm ci --prefix app`); no app success is claimed']
    legs=[('Svelte app check',[npm,'--prefix','app','run','check']),('Svelte app unit tests',[npm,'--prefix','app','run','test']),('Svelte app build',[npm,'--prefix','app','run','build'])]
    gap=e2e_gap()
    if gap:return legs,[f'UNVERIFIED Svelte app e2e: {gap}; no e2e success is claimed']
    return legs+[('Svelte app Playwright e2e',[sys.executable,'scripts/test_app.py'])],[]
def run_app(legs,run,env):
    """Run the app legs in order and stop at the first failure; True when every leg passed."""
    for name,command in legs:
        print('\nRUN '+name,flush=True)
        if run(command,cwd=ROOT,env=env,check=False).returncode:
            print('FAIL '+name,flush=True);print('STOP Svelte app legs after the first failure',flush=True);return False
        print('PASS '+name,flush=True)
    return True
def main(argv=None,*,which=shutil.which,run=subprocess.run,isdir=os.path.isdir,e2e_gap=playwright_gap):
    p=argparse.ArgumentParser();p.add_argument('--require-native',action='store_true');p.add_argument('--ui',action='store_true');p.add_argument('--browser');p.add_argument('--app',action='store_true',help='also run the Svelte app checks, build and Playwright e2e (app/)');a=p.parse_args(argv)
    env=dict(os.environ,PYTHONPATH=str(ROOT/'python'));failed=False
    commands=[('Python suite',[sys.executable,'-m','unittest','discover','-s','tests'])]
    if which('node'):
        commands.extend([('JavaScript app syntax',['node','--check','ui/app.js']),
                         ('JavaScript expansion syntax',['node','--check','ui/expansion.js']),
                         ('JavaScript boundary and Python parity fixtures',['node','tests/test_ui_logic.mjs'])])
    else:print('SKIP JavaScript syntax: Node is missing',flush=True)
    if which('cargo'):
        commands.extend([('Rust format',['cargo','fmt','--all','--','--check']),('Rust unit tests',['cargo','test','--workspace']),('Rust lint',['cargo','clippy','--workspace','--all-targets','--','-D','warnings'])])
    else:
        print('UNVERIFIED Rust: cargo is missing; no native success is claimed',flush=True)
        failed=a.require_native
    if a.ui:
        c=[sys.executable,'scripts/test_ui.py'];c+=['--browser',a.browser] if a.browser else [];commands.append(('Chromium bridged-UI integration',c))
    legs,notes=app_legs(ROOT,which,isdir,e2e_gap) if a.app else ([],[])
    for line in notes:print(line,flush=True)
    for name,command in commands:
        print('\nRUN '+name,flush=True)
        result=run(command,cwd=ROOT,env=env,check=False)
        if result.returncode:failed=True;print('FAIL '+name,flush=True)
        else:print('PASS '+name,flush=True)
    if not run_app(legs,run,env):failed=True
    return 1 if failed else 0
if __name__=='__main__':raise SystemExit(main())
