"""Explicit, local environment setup. No global installers or automatic network calls."""
from __future__ import annotations
import argparse
import json
import platform
import shutil
import subprocess
import sys
import venv
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def main():
    p=argparse.ArgumentParser();p.add_argument('--create-venv',action='store_true');p.add_argument('--telemetry',action='store_true');a=p.parse_args()
    if sys.version_info<(3,11):raise SystemExit('Python 3.11+ is required')
    print(json.dumps({'python':platform.python_version(),'git':bool(shutil.which('git')),'cargo':bool(shutil.which('cargo')),'node':bool(shutil.which('node'))},indent=2))
    env=ROOT/'.venv';python=env/('Scripts/python.exe' if sys.platform=='win32' else 'bin/python')
    if a.create_venv:
        if env.exists():raise SystemExit('Refusing to replace an existing .venv. Use it as-is or select a clean checkout.')
        venv.EnvBuilder(with_pip=True).create(env);print('Created repository-local .venv')
    if a.telemetry:
        if not python.is_file():raise SystemExit('Create the .venv first; telemetry will not be installed globally')
        subprocess.run([str(python),'-m','pip','install','psutil==7.2.2'],check=True,timeout=300)
    print('Run: python scripts/run.py demo --open')
    print('Native path: install Rust MSVC and Microsoft C++ Build Tools through their official installers, then run cargo test --workspace.')
    return 0
if __name__=='__main__':raise SystemExit(main())
