"""Launch only an explicitly chosen synthetic or exported snapshot MCP view."""
from __future__ import annotations
import argparse
import sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'python'))
from loomward.catalog import Catalog
from loomward.interop import ToolService
from loomward.mcp_stdio import Protocol,serve
from loomward.teacher import strict_json


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    group=parser.add_mutually_exclusive_group(required=True)
    group.add_argument('--demo',action='store_true')
    group.add_argument('--snapshot',type=Path,help='Read this exported JSON only; never scan the paths it contains.')
    parser.add_argument('--scope-prefix',default='',help='Optional component-bound relative prefix within the snapshot.')
    parser.add_argument('--disclose-names',action='store_true',help='Explicitly permit returned names and relative paths. Your MCP host may send them to its model.')
    args=parser.parse_args()
    source=ROOT/'fixtures/demo.json' if args.demo else args.snapshot
    try:
        # Read the owner-selected file with a strict byte bound; no path from its contents is opened.
        with source.open('rb') as stream: raw=stream.read(20*1024*1024+1)
        if len(raw)>20*1024*1024: raise ValueError('Snapshot exceeds the 20 MiB import limit')
        data=strict_json(raw.decode('utf-8'))
        snapshot=data['inventory'] if args.demo else data
        with Catalog(snapshot,prefix=args.scope_prefix,disclose_names=args.disclose_names) as catalog:
            service=ToolService(catalog,origin='synthetic_demo' if args.demo else 'supplied_snapshot')
            print('Loomward snapshot reference. No file content, network or execution tools. Returned metadata follows the host privacy policy.',file=sys.stderr)
            serve(Protocol(service),sys.stdin.buffer,sys.stdout.buffer)
        return 0
    except BrokenPipeError:
        return 0
    except (OSError,ValueError,KeyError,TypeError,RecursionError):
        print('Cannot open or validate the explicitly selected snapshot; no MCP server started.',file=sys.stderr)
        return 2
if __name__=='__main__':raise SystemExit(main())
