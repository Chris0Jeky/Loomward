"""Run available verification gates; missing native tools are never called a pass.

Flags:
  --require-native  fail the run when cargo is missing
  --app             also run the Svelte app in app/: check, unit tests, build, then the
                    Playwright e2e (scripts/test_app.py), stopping at the first failure;
                    reports UNVERIFIED when Node/npm or app/node_modules is missing, and
                    UNVERIFIED for only the e2e when the Playwright package or its Chromium
                    is missing (#154, #186, #431). An explicit --app request returns
                    status 2 if any required app leg is unverified; executed failures
                    return 1 and take precedence. Plain verification keeps app optional.
  --changed [BASE]  local loop only (#1016): test and lint just the workspace crates a diff
                    (BASE...HEAD plus uncommitted and untracked files; default origin/main) can
                    affect, plus the Python suite when Python inputs changed. Falls back to the
                    full workspace when it cannot attribute a change. A selected run is never
                    the full suite: hosted CI runs the whole workspace.
"""
import argparse
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path, PurePath
from typing import NamedTuple
ROOT=Path(__file__).resolve().parents[1]
# --- --changed scope selection (#1016). Pure functions first; the I/O is gather_scope. ---
FALLBACK_DIRS=('contracts/','fixtures/','schemas/','.cargo/')  # read by crates from outside any crate
FALLBACK_FILES=('Cargo.toml','Cargo.lock','rustfmt.toml','.rustfmt.toml','clippy.toml','.clippy.toml')
INERT_DIRS=('app/','backlog/','bucket/','docs/','evidence/','experiments/','handoff/','native/','packaging/','python/','scripts/','tests/','.github/','.agent-harness/')
PYTHON_DIRS=('python/','tests/','fixtures/','contracts/','scripts/','schemas/','backlog/','docs/','native/','packaging/','handoff/','app/')  # the Python tests read these (grep, 2026-10-30)
PYTHON_FILES=('pyproject.toml',)
STRING=re.compile(r'"((?:[^"\\\n]|\\.)*)"')
TREE_SCAN=re.compile(r'\.parent\(\)\s*\.unwrap\(\)\s*\.parent\(\)|join\("\.\./\.\."\)')
PY_CALL=re.compile(r'Command::new\("(?:py|python3?)"\)')
class Scope(NamedTuple):
    full:bool;crates:tuple;reasons:tuple;python:bool;native:bool;scanners:tuple
def norm_ref(text):
    return re.sub(r'^(?:\.{1,2}/|/)+','',text.replace('\\','/'))
def scan_sources(texts):
    """Return (path-like string literals, reads the workspace tree, shells out to Python) for one crate's .rs texts.
    ponytail: a text heuristic, not a Rust parser; a path built at run time from pieces that
    are not literals is invisible to it. Hosted CI runs the whole workspace and is the backstop."""
    lits=set();scans=False;py=False
    for text in texts:
        for raw in STRING.findall(text):
            ref=norm_ref(raw)
            if len(ref)>2 and ' ' not in ref and ('/' in ref or '.' in ref):lits.add(ref)
        scans=scans or ('read_dir' in text and bool(TREE_SCAN.search(text)))
        py=py or bool(PY_CALL.search(text))
    return lits,scans,py
def names(lits,path):
    """True when a literal from a crate can be the file or a directory holding it."""
    return any(path==l or ('/' in l and (path.endswith('/'+l) or path.startswith(l.rstrip('/')+'/'))) for l in lits)
def select_scope(changed,packages,workspace_root,reads):
    """Map changed repo-relative files to workspace crates and close over dependents.

    packages: `cargo metadata --no-deps` packages; reads: {crate: scan_sources(...)}."""
    root=PurePath(workspace_root)
    dirs={PurePath(p['manifest_path']).parent.relative_to(root).as_posix():p['name'] for p in packages}
    members=set(dirs.values());rdeps={n:set() for n in members}
    for p in packages:
        for d in p['dependencies']:
            if d.get('path') and d['name'] in members:rdeps[d['name']].add(p['name'])
    scanners=tuple(sorted(n for n,r in reads.items() if r[1]));pyrunners=sorted(n for n,r in reads.items() if r[2])
    seeds=set();why=[];full=[];python=False;native=False;rust_text=False
    for f in (c.replace('\\','/') for c in changed):
        python=python or f.startswith(PYTHON_DIRS) or f in PYTHON_FILES
        native=native or f.startswith('native/')
        rust_text=rust_text or f.endswith(('.rs','Cargo.toml'))
        owner=next((dirs[d] for d in sorted(dirs,key=len,reverse=True) if f.startswith(d+'/')),None)
        readers=sorted(n for n,r in reads.items() if n!=owner and names(r[0],f))+(pyrunners if f.startswith('python/') else [])
        if owner:seeds.add(owner);why.append(f'{f} -> crate {owner}')
        elif f.startswith('crates/'):full.append(f'{f}: inside crates/ but in no workspace member')
        elif f in FALLBACK_FILES or f.startswith(FALLBACK_DIRS) or f.startswith('rust-toolchain'):full.append(f'{f}: shared build or data input')
        elif readers:why.append(f'{f} -> read by crate {", ".join(readers)}')
        elif f.startswith(INERT_DIRS) or '/' not in f:why.append(f'{f} -> no crate reads it')
        else:full.append(f'{f}: cannot attribute this path to a crate')
        if owner and readers:why.append(f'{f} -> also read by crate {", ".join(readers)}')
        seeds.update(readers)
    if scanners and (seeds or rust_text):
        seeds.update(scanners);why.append('tree-scanning crate(s) always join a Rust change: '+', '.join(scanners))
    todo=list(seeds)
    while todo:
        for dep in rdeps[todo.pop()]:
            if dep not in seeds:seeds.add(dep);todo.append(dep)
    return Scope(bool(full),tuple(sorted(members if full else seeds)),tuple(full+why),python,native,scanners)
def gather_scope(base,run=subprocess.run,root=ROOT):
    """Real I/O for --changed: returns (Scope, None) or (None, why the scope could not be computed)."""
    def out(cmd):
        r=run(cmd,cwd=root,capture_output=True,text=True,check=False)
        if r.returncode:raise RuntimeError(f'{" ".join(cmd[:2])} failed: {(r.stderr or "").strip()[:200]}')
        return r.stdout
    try:
        files=set(out(['git','diff','--name-only',f'{base}...HEAD']).split('\n'))
        files|=set(out(['git','diff','--name-only','HEAD']).split('\n'))|set(out(['git','ls-files','--others','--exclude-standard']).split('\n'))
        meta=json.loads(out(['cargo','metadata','--no-deps','--format-version','1']))
        reads={}
        for p in meta['packages']:
            texts=[f.read_text(encoding='utf-8',errors='replace') for f in Path(p['manifest_path']).parent.rglob('*.rs') if 'target' not in f.parts]
            reads[p['name']]=scan_sources(texts)
        return select_scope(sorted(f for f in files if f),meta['packages'],meta['workspace_root'],reads),None
    except (RuntimeError,OSError,ValueError,KeyError) as e:return None,str(e)
def scope_summary(scope,base,why=None):
    """Lines that state exactly what a --changed run covered; a selection is never called the workspace."""
    if scope is None:return [f'SCOPE full workspace and Python suite (cannot compute the change scope: {why})']
    lines=[f'SELECTION against {base}:']+['  '+r for r in scope.reasons]
    python='Python suite: '+('ran' if scope.python else 'not run (no Python input changed)')
    if scope.full:lines.append('SCOPE Rust: full workspace (fallback above); '+python)
    else:
        lines.append(f'SCOPE Rust: selected crates only ({", ".join(scope.crates) or "none"}); hosted CI runs the whole workspace. Only cargo fmt covered the whole workspace. '+python)
    if scope.native:lines.append('native/ changed; native was not run (excluded from the workspace)')
    return lines
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
def main(argv=None,*,which=shutil.which,run=subprocess.run,isdir=os.path.isdir,e2e_gap=playwright_gap,scope_for=gather_scope):
    p=argparse.ArgumentParser();p.add_argument('--require-native',action='store_true');p.add_argument('--app',action='store_true',help='require Svelte app checks, build and Playwright e2e; exit 2 if unverified')
    p.add_argument('--changed',nargs='?',const='origin/main',metavar='BASE',help='local loop: only the crates BASE...HEAD (default origin/main) can affect; hosted CI runs the whole workspace');a=p.parse_args(argv)
    env=dict(os.environ,PYTHONPATH=str(ROOT/'python'));failed=False
    commands=[('Python suite',[sys.executable,'-m','unittest','discover','-s','tests'])]
    scope=why=None;summary=[]
    if a.changed and which('cargo'):
        scope,why=scope_for(a.changed);summary=scope_summary(scope,a.changed,why)
        for line in summary:print(line,flush=True)
    if scope and not scope.python:commands=[]
    if which('cargo'):
        if scope and not scope.full:
            pkgs=[x for c in scope.crates for x in ('-p',c)]
            commands.append(('Rust format (whole workspace)',['cargo','fmt','--all','--','--check']))
            if pkgs:commands.extend([('Rust unit tests (selected crates)',['cargo','test',*pkgs,'--no-fail-fast']),('Rust lint (selected crates)',['cargo','clippy',*pkgs,'--all-targets','--','-D','warnings'])])
        else:commands.extend([('Rust format',['cargo','fmt','--all','--','--check']),('Rust unit tests',['cargo','test','--workspace','--no-fail-fast']),('Rust lint',['cargo','clippy','--workspace','--all-targets','--','-D','warnings'])])
    else:
        print('UNVERIFIED Rust: cargo is missing; no native success is claimed',flush=True)
        failed=a.require_native
    legs,notes=app_legs(ROOT,which,isdir,e2e_gap) if a.app else ([],[])
    for line in notes:print(line,flush=True)
    for name,command in commands:
        print('\nRUN '+name,flush=True)
        result=run(command,cwd=ROOT,env=env,check=False)
        if result.returncode:failed=True;print('FAIL '+name,flush=True)
        else:print('PASS '+name,flush=True)
    if not run_app(legs,run,env):failed=True
    if summary:print('\n'+'\n'.join(l for l in summary if l.startswith(('SCOPE','native/'))),flush=True)  # restate the scope last so a log tail cannot read as a full pass
    # Missing optional Rust does not fail plain verification. An explicitly requested
    # app gate is different: successful available legs do not qualify the missing legs.
    return 1 if failed else (2 if notes else 0)
if __name__=='__main__':raise SystemExit(main())
