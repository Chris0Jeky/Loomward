"""Run available verification gates; missing native tools are never called a pass."""
import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def main():
    p=argparse.ArgumentParser();p.add_argument('--require-native',action='store_true');p.add_argument('--ui',action='store_true');p.add_argument('--browser');a=p.parse_args()
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
    for name,command in commands:
        print('\nRUN '+name,flush=True)
        result=subprocess.run(command,cwd=ROOT,env=env,check=False)
        if result.returncode:failed=True;print('FAIL '+name,flush=True)
        else:print('PASS '+name,flush=True)
    return 1 if failed else 0
if __name__=='__main__':raise SystemExit(main())
