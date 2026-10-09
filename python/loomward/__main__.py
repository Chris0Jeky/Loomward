"""Source-checkout CLI. Observation and simulation only; no action executor."""
from __future__ import annotations
import argparse
import importlib.util
import json
import os
import platform
import shutil
import sys
import webbrowser
from pathlib import Path
from typing import Any
from . import __version__
from .inventory import scan, checked_root
from .duplicates import find_duplicates
from .learning import Student, DEFAULT_LABELS
from .planner import plan_tiers
from .teacher import strict_json, build_request, request_teacher
from .telemetry import Telemetry


def read_json(path: str | Path, *, limit: int = 20 * 1024**2) -> Any:
    with Path(path).open('rb') as f:
        data = f.read(limit + 1)
    if len(data) > limit:
        raise ValueError('Input exceeds the 20 MiB prototype limit')
    return strict_json(data.decode('utf-8'))


def emit(value: Any, output: str | None) -> None:
    text = json.dumps(value, ensure_ascii=True, allow_nan=False, indent=2) + '\n'
    if output:
        target = Path(output).absolute()
        checked_root(target.parent)
        # Exclusive create prevents accidental overwrite, including a symlink target.
        with target.open('x', encoding='utf-8') as f:
            f.write(text)
    else:
        print(text, end='')


def default_state() -> Path:
    if os.name == 'nt':
        base = Path(os.environ.get('LOCALAPPDATA', Path.home() / 'AppData' / 'Local'))
    else:
        base = Path(os.environ.get('XDG_STATE_HOME', Path.home() / '.local' / 'state'))
    return base / 'Loomward-reference'


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog='loomward', description='Local-first observation, learning and storage simulation. No file or process mutation commands.')
    p.add_argument('--version', action='version', version=__version__)
    sub = p.add_subparsers(dest='command', required=True)
    for name in ('demo', 'serve'):
        s = sub.add_parser(name, help='Run the loopback reference UI; demo uses synthetic data')
        if name == 'serve':
            s.add_argument('--root', required=True, help='Explicit metadata-scan root; no implicit whole-disk scan')
            s.add_argument('--processes', action='store_true', help='Permit optional read-only process inspection')
            s.add_argument('--max-entries', type=int, default=50_000)
        s.add_argument('--state-dir', type=Path, default=None)
        s.add_argument('--port', type=int, default=8765)
        s.add_argument('--open', action='store_true', help='Open the printed local session in the default browser')
    s=sub.add_parser('doctor', help='Print capability and tool availability, without a file scan');s.add_argument('--output')
    s=sub.add_parser('scan', help='Read bounded metadata only');s.add_argument('root');s.add_argument('--max-entries',type=int,default=50_000);s.add_argument('--output')
    s=sub.add_parser('duplicates', help='Explicitly read contents within a budget; never delete')
    s.add_argument('root');s.add_argument('--snapshot',required=True);s.add_argument('--consent-content',action='store_true');s.add_argument('--byte-budget',type=int,default=256*1024**2);s.add_argument('--output')
    s=sub.add_parser('plan', help='Simulate a user-supplied tier scenario');s.add_argument('scenario');s.add_argument('--output')
    s=sub.add_parser('train', help='Fit an inspectable student from feedback JSON');s.add_argument('feedback');s.add_argument('--allow-synthetic',action='store_true');s.add_argument('--output')
    s=sub.add_parser('predict', help='Suggest labels without taking actions');s.add_argument('model');s.add_argument('features');s.add_argument('--output')
    for name in ('teacher-request','teacher'):
        s=sub.add_parser(name, help='Build an offline request' if name=='teacher-request' else 'Send explicitly approved metadata to a local teacher')
        s.add_argument('features');s.add_argument('--model',required=True);s.add_argument('--item-id',required=True);s.add_argument('--output')
        if name=='teacher':
            s.add_argument('--endpoint',default='http://127.0.0.1:1234/v1/chat/completions');s.add_argument('--consent-metadata',action='store_true')
    s=sub.add_parser('processes',help='Take a single read-only process sample; first CPU rate is unknown');s.add_argument('--output')
    return p


def main(argv: list[str] | None = None) -> int:
    args=parser().parse_args(argv)
    try:
        if args.command in ('demo','serve'):
            from .server import App, Server
            if not 0 <= args.port <= 65535:
                raise ValueError('Port must be in 0..65535; use 0 to select a free port')
            demo=args.command=='demo';state=args.state_dir or default_state()
            app=App(state/('demo.db' if demo else 'observed.db'),demo=demo,
                    root=None if demo else args.root,allow_processes=getattr(args,'processes',False),
                    max_entries=getattr(args,'max_entries',50_000))
            server=Server(('127.0.0.1',args.port),app)
            url=server.origin+'/#token='+server.token
            print('Loomward reference UI. '+('SYNTHETIC DEMO.' if demo else 'EXPLICIT ROOT OBSERVATION.'))
            print('No user-file or process mutation endpoints. Ctrl+C stops the local session.')
            print(url,flush=True)
            if args.open:webbrowser.open(url)
            try:server.serve_forever()
            except KeyboardInterrupt:pass
            finally:server.server_close()
            return 0
        if args.command=='doctor':
            result={'version':__version__,'python':platform.python_version(),'platform':platform.system(),
                    'runtime':'python_reference','rust_toolchain_available':bool(shutil.which('cargo')),
                    'node_available':bool(shutil.which('node')),'optional_telemetry_available':importlib.util.find_spec('psutil') is not None,
                    'capabilities':{'file_mutation':False,'process_mutation':False,'bounded_metadata_scan':True,
                                    'explicit_duplicate_reads':True,'learned_label_suggestions':True,'tier_simulation':True},
                    'native_status':'Rust/Tauri sources are not certified by this Python doctor command.'}
        elif args.command=='scan':result=scan(args.root,max_entries=args.max_entries)
        elif args.command=='duplicates':
            if not args.consent_content:raise ValueError('Add --consent-content to authorise bounded content reads')
            result=find_duplicates(args.root,read_json(args.snapshot),byte_budget=args.byte_budget)
        elif args.command=='plan':result=plan_tiers(read_json(args.scenario))
        elif args.command=='train':
            value=read_json(args.feedback)
            if not isinstance(value,dict) or not isinstance(value.get('events'),list) or value.get('mode') not in ('observed','imported','demo'):
                raise ValueError('Feedback must be an object with events and explicit observed/imported/demo mode')
            synthetic=value['mode']=='demo'
            if synthetic and not args.allow_synthetic:raise ValueError('Demo training requires --allow-synthetic; do not mix demo labels into a personal profile')
            result=Student(value.get('labels',DEFAULT_LABELS)).fit(value['events']).to_dict();result['synthetic_training']=synthetic
        elif args.command=='predict':
            model=read_json(args.model);result=Student.from_dict(model).predict(read_json(args.features));result['synthetic_training']=model.get('synthetic_training',False)
        elif args.command=='teacher-request':result=build_request(args.item_id,read_json(args.features),DEFAULT_LABELS,args.model)
        elif args.command=='teacher':result=request_teacher(args.endpoint,args.model,args.item_id,read_json(args.features),DEFAULT_LABELS,consent_metadata=args.consent_metadata)
        elif args.command=='processes':result=Telemetry().snapshot()
        else:raise ValueError('Unknown command')
        emit(result,getattr(args,'output',None));return 0
    except (ValueError,OSError,TypeError,KeyError) as exc:
        print(f'loomward: {exc}',file=sys.stderr);return 2

if __name__=='__main__':
    raise SystemExit(main())
