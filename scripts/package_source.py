"""Create a checksummed source ZIP, excluding runtimes, private state and Git internals.
Run after review/testing. The generated manifest is deliberately not self-hashed.
"""
from __future__ import annotations
import argparse
import hashlib
from pathlib import Path
import zipfile
ROOT=Path(__file__).resolve().parents[1]
EXCLUDED={'.git','.venv','__pycache__','node_modules','target','dist','.loomward','.superpowers','.pytest_cache'}
def main():
    p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    output=a.output.absolute()
    if output==ROOT or ROOT in output.parents:raise SystemExit('Select an output ZIP outside the source tree')
    if output.exists():raise SystemExit('Refusing to overwrite an existing artifact')
    files=[]
    for f in sorted(ROOT.rglob('*')):
        rel=f.relative_to(ROOT)
        if any(x in EXCLUDED for x in rel.parts) or not f.is_file():continue
        if f.is_symlink():raise SystemExit('Source package cannot contain symlinks')
        if f.name=='MANIFEST.sha256':continue
        if f.name=='.env' or f.suffix.lower() in {'.db','.sqlite','.sqlite3','.pem','.pfx','.key','.pyc','.zip','.bundle'}:raise SystemExit('Unexpected state/private/archive file in source tree: '+str(rel))
        files.append(f)
    manifest=ROOT/'MANIFEST.sha256'
    manifest.write_text(''.join(hashlib.sha256(f.read_bytes()).hexdigest()+'  '+f.relative_to(ROOT).as_posix()+'\n' for f in files),encoding='utf-8')
    files.append(manifest)
    with zipfile.ZipFile(output,'x',compression=zipfile.ZIP_DEFLATED,compresslevel=9) as z:
        for f in sorted(files):
            info=zipfile.ZipInfo('loomward/'+f.relative_to(ROOT).as_posix(),date_time=(2026,10,9,0,0,0))
            info.compress_type=zipfile.ZIP_DEFLATED;info.external_attr=0o100644<<16
            z.writestr(info,f.read_bytes())
    with zipfile.ZipFile(output) as z:
        if z.testzip() is not None:raise SystemExit('ZIP integrity verification failed')
    print(f'Created {output.name}: {len(files)} source entries, SHA-256 '+hashlib.sha256(output.read_bytes()).hexdigest())
if __name__=='__main__':main()
