"""Create a checksummed source ZIP, excluding runtimes, private state and Git internals.
Run after review/testing. The generated manifest is deliberately not self-hashed.
"""
from __future__ import annotations
import argparse
import hashlib
import subprocess
from pathlib import Path
import zipfile
ROOT=Path(__file__).resolve().parents[1]
EXCLUDED={'.git','.venv','__pycache__','node_modules','target','dist','.loomward','.superpowers','.pytest_cache'}
def source_files(root: Path) -> list[Path]:
    try:
        proc=subprocess.run(['git','-C',str(root),'ls-files','-z'],capture_output=True,check=True)
    except (OSError,subprocess.CalledProcessError):
        raise SystemExit('Cannot list tracked files: '+str(root)+' is not a git checkout or git is unavailable')
    top=subprocess.run(['git','-C',str(root),'rev-parse','--show-toplevel'],capture_output=True,text=True)
    if top.returncode or Path(top.stdout.strip()).resolve()!=root.resolve():
        raise SystemExit('Package from the repository root: '+str(root)+' is not the top of a git checkout')
    untracked=subprocess.run(['git','-C',str(root),'ls-files','--others','--exclude-standard','-z'],capture_output=True,check=True).stdout
    if untracked:
        raise SystemExit('Untracked source files would be silently left out; add or ignore them first')
    files=[]
    for name in sorted(n.decode('utf-8','surrogateescape') for n in proc.stdout.split(b'\0') if n):
        rel=Path(name)
        f=root/rel
        if any(x in EXCLUDED for x in rel.parts) or not f.is_file():continue
        if f.is_symlink():raise SystemExit('Source package cannot contain symlinks')
        if f.name=='MANIFEST.sha256':continue
        if f.name=='.env' or f.suffix.lower() in {'.db','.sqlite','.sqlite3','.pem','.pfx','.key','.pyc','.zip','.bundle'}:raise SystemExit('Unexpected state/private/archive file in source tree: '+str(rel))
        files.append(f)
    return files
def main():
    p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    output=a.output.absolute()
    if output==ROOT or ROOT in output.parents:raise SystemExit('Select an output ZIP outside the source tree')
    if output.exists():raise SystemExit('Refusing to overwrite an existing artifact')
    files=source_files(ROOT)
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
