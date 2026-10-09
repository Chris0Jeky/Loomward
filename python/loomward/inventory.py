"""Bounded, metadata-only inventory. A snapshot is evidence, never action authority."""
from __future__ import annotations
import hashlib
import os
import stat
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SCHEMA_VERSION = 1
MAX_RECORDS = 200_000
REPARSE = 0x400
OFFLINE = 0x1000
RECALL_ON_OPEN = 0x40000
RECALL_ON_DATA_ACCESS = 0x400000
DEFAULT_EXCLUDED = frozenset({'.loomward', '$recycle.bin', 'system volume information'})
SENSITIVE_SUFFIXES = {'.key', '.pem', '.pfx', '.p12', '.kdbx'}
PROTECTED_PARTS = {'.git', '.hg', '.svn', 'windows', 'program files', 'program files (x86)', 'appdata'}


def integer(value: Any, name: str, minimum: int = 0, maximum: int = 2**53 - 1) -> int:
    if type(value) is not int or not minimum <= value <= maximum:
        raise ValueError(f'{name} must be an integer in [{minimum}, {maximum}]')
    return value


def linklike(meta: os.stat_result) -> bool:
    attrs = getattr(meta, 'st_file_attributes', 0)
    return stat.S_ISLNK(meta.st_mode) or bool(attrs & (REPARSE | OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS))


def checked_root(root: str | Path) -> Path:
    """Reject linked/reparse ancestors before normalising; resolve() alone would hide them."""
    p = Path(os.path.abspath(os.path.expanduser(os.fspath(root))))
    try:
        for part in reversed((p, *p.parents)):
            if linklike(os.lstat(part)):
                raise ValueError(f'Linked, reparse or offline root component is not supported: {part}')
        if not stat.S_ISDIR(os.lstat(p).st_mode):
            raise ValueError('Scan root must be a directory')
    except OSError as exc:
        raise ValueError(f'Root cannot be inspected: {exc}') from exc
    return p


def sensitive_name(name: str) -> bool:
    n = name.casefold()
    return n == '.env' or n.startswith('.env.') or n.startswith('id_rsa') or n.startswith('id_ed25519') or Path(n).suffix in SENSITIVE_SUFFIXES


def change_ns(meta: os.stat_result) -> int:
    # Windows 3.12+: stat() reports creation time as st_ctime but fstat() reports change time; birthtime agrees.
    return getattr(meta, 'st_birthtime_ns', meta.st_ctime_ns) if os.name == 'nt' else meta.st_ctime_ns


def signature(meta: os.stat_result) -> tuple[int, ...]:
    return (meta.st_dev, meta.st_ino, meta.st_size, meta.st_mtime_ns, change_ns(meta))


def scan(root: str | Path, *, max_entries: int = 50_000, max_depth: int = 64,
         exclude_names: frozenset[str] = DEFAULT_EXCLUDED) -> dict[str, Any]:
    integer(max_entries, 'max_entries', 1, MAX_RECORDS)
    integer(max_depth, 'max_depth', 0, 256)
    exclude_names = frozenset(n.casefold() for n in exclude_names)
    root = checked_root(root)
    started = time.monotonic()
    records: list[dict[str, Any]] = []
    errors: list[dict[str, str]] = []
    skipped: list[dict[str, str]] = []
    examined = skipped_count = error_count = directories = 0
    limit_hit = False
    stack = [(root, 0)]
    root_key = hashlib.sha256(os.fsencode(str(root))).hexdigest()[:20]

    def skip(path: Path, reason: str) -> None:
        nonlocal skipped_count
        skipped_count += 1
        if len(skipped) < 128:
            skipped.append({'path': str(path.relative_to(root)), 'reason': reason})

    def error(path: Path, exc: OSError) -> None:
        nonlocal error_count
        error_count += 1
        if len(errors) < 128:
            errors.append({'path': str(path.relative_to(root)), 'reason': str(exc)})

    while stack and not limit_hit:
        directory, depth = stack.pop()
        try:
            if linklike(os.lstat(directory)):
                skip(directory, 'changed_to_link_or_placeholder')
                continue
            with os.scandir(directory) as entries:
                for entry in entries:
                    if examined >= max_entries:
                        limit_hit = True
                        break
                    examined += 1
                    path = Path(entry.path)
                    try:
                        meta = entry.stat(follow_symlinks=False)
                        if os.name == 'nt' and stat.S_ISREG(meta.st_mode):
                            meta = os.lstat(path)  # Windows DirEntry.stat() leaves st_ino/st_dev/st_nlink at 0
                        if linklike(meta):
                            skip(path, 'link_reparse_or_cloud_placeholder')
                            continue
                        if entry.name.casefold() in exclude_names:
                            skip(path, 'policy_excluded')
                            continue
                        if stat.S_ISDIR(meta.st_mode):
                            directories += 1
                            if depth >= max_depth:
                                skip(path, 'depth_limit')
                            else:
                                stack.append((path, depth + 1))
                            continue
                        if not stat.S_ISREG(meta.st_mode):
                            skip(path, 'special_file')
                            continue
                        rel = path.relative_to(root).as_posix()
                        flags = []
                        if sensitive_name(entry.name):
                            flags.append('sensitive')
                        if any(p.casefold() in PROTECTED_PARTS for p in path.relative_to(root).parts):
                            flags.append('protected_context')
                        if meta.st_nlink > 1:
                            flags.append('hardlinked')
                        allocated = getattr(meta, 'st_blocks', None)
                        allocated = allocated * 512 if allocated is not None else None
                        if allocated is not None and allocated < meta.st_size:
                            flags.append('sparse_or_compressed')
                        records.append({
                            'id': hashlib.sha256((root_key + '\0' + rel).encode('utf-8', 'surrogatepass')).hexdigest()[:24],
                            'relative_path': rel, 'name': entry.name, 'extension': path.suffix.casefold(),
                            'size_bytes': meta.st_size, 'allocated_bytes': allocated,
                            'mtime_ns': meta.st_mtime_ns, 'ctime_ns': change_ns(meta),
                            'device': meta.st_dev, 'inode': meta.st_ino, 'nlink': meta.st_nlink,
                            'flags': flags, 'identity_quality': 'portable_stat_observation',
                        })
                    except OSError as exc:
                        error(path, exc)
        except OSError as exc:
            error(directory, exc)
    records.sort(key=lambda f: f['relative_path'])
    allocated_known = sum(f['allocated_bytes'] for f in records if f['allocated_bytes'] is not None)
    unknown = sum(f['allocated_bytes'] is None for f in records)
    return {
        'schema_version': SCHEMA_VERSION, 'mode': 'observed', 'root': str(root), 'root_key': root_key,
        'observed_at': datetime.now(timezone.utc).isoformat(), 'files': records,
        'summary': {'file_count': len(records), 'directory_count': directories,
                    'logical_bytes': sum(f['size_bytes'] for f in records),
                    'allocated_bytes_known_sum': allocated_known, 'allocated_unknown_count': unknown,
                    'allocation_note': 'Per-entry observations; hard links can be counted more than once. Not total volume usage.',
                    'elapsed_seconds': round(time.monotonic() - started, 6)},
        'coverage': {'complete_under_policy': not limit_hit and error_count == 0,
                     'unqualified_complete': not limit_hit and error_count == 0 and skipped_count == 0,
                     'limit_hit': limit_hit, 'examined_entries': examined,
                     'skipped_count': skipped_count, 'error_count': error_count,
                     'skipped': skipped, 'errors': errors, 'max_entries': max_entries, 'max_depth': max_depth},
        'capabilities': {'metadata_scan': True, 'content_hash_requires_consent': True,
                         'file_mutation': False, 'process_mutation': False},
    }


def features_for(record: dict[str, Any]) -> dict[str, Any]:
    """A minimal metadata feature set; raw content is deliberately absent."""
    parts = record['relative_path'].split('/')
    return {'name': record['name'][:256], 'extension': record.get('extension', '')[:32],
            'context': ' '.join(parts[-3:-1])[:512], 'size_bytes': record['size_bytes']}
