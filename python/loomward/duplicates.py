"""Opt-in content inspection with a global read budget. No delete operation exists."""
from __future__ import annotations
import hashlib
import os
import stat
from collections import defaultdict
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Iterator, BinaryIO
from .inventory import checked_root, integer, linklike, signature

CHUNK = 1024 * 1024


def checked_child(root: Path, relative: str) -> Path:
    if not isinstance(relative, str) or not relative or '\\' in relative or '\x00' in relative:
        raise ValueError('Invalid relative path')
    parts = relative.split('/')
    if any(p in ('', '.', '..') or ':' in p for p in parts):
        raise ValueError('Invalid path component')
    p = root
    for part in parts:
        p = p / part
        if linklike(os.lstat(p)):
            raise ValueError('Linked, reparse, offline or placeholder path')
    return p


def expected_signature(record: dict[str, Any]) -> tuple[int, ...]:
    return tuple(record[k] for k in ('device', 'inode', 'size_bytes', 'mtime_ns', 'ctime_ns'))


@contextmanager
def stable_open(root: Path, record: dict[str, Any]) -> Iterator[BinaryIO]:
    path = checked_child(root, record['relative_path'])
    flags = os.O_RDONLY | getattr(os, 'O_BINARY', 0) | getattr(os, 'O_NOFOLLOW', 0) | getattr(os, 'O_NONBLOCK', 0)
    fd = os.open(path, flags)
    try:
        with os.fdopen(fd, 'rb') as stream:
            fd = -1
            before = os.fstat(stream.fileno())
            if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or signature(before) != expected_signature(record):
                raise ValueError('File changed or is not an eligible ordinary file')
            yield stream
            after = os.fstat(stream.fileno())
            path = checked_child(root, record['relative_path'])
            if signature(after) != expected_signature(record) or signature(os.lstat(path)) != signature(after):
                raise ValueError('File or path changed during content inspection')
    finally:
        if fd >= 0:
            os.close(fd)


def find_duplicates(root: str | Path, inventory: dict[str, Any], *, byte_budget: int = 256 * 1024**2) -> dict[str, Any]:
    integer(byte_budget, 'byte_budget', 0, 2 * 1024**3)
    root = checked_root(root)
    if inventory.get('mode') != 'observed' or os.path.normcase(str(root)) != os.path.normcase(str(inventory.get('root'))):
        raise ValueError('Inventory belongs to a different root or is not observed data')
    by_size: dict[int, list[dict[str, Any]]] = defaultdict(list)
    skipped: list[dict[str, str]] = []
    for r in inventory.get('files', []):
        if r.get('size_bytes', 0) <= 0:
            continue
        if r.get('nlink') != 1 or set(r.get('flags', [])) & {'sensitive', 'protected_context', 'hardlinked'}:
            skipped.append({'path': r.get('relative_path', ''), 'reason': 'protected_or_hardlinked'})
            continue
        by_size[r['size_bytes']].append(r)
    read_bytes = 0
    exhausted = False
    digests: dict[tuple[int, str], list[dict[str, Any]]] = defaultdict(list)
    for size, rows in sorted(by_size.items()):
        if len(rows) < 2:
            continue
        for r in rows:
            if size > byte_budget - read_bytes:
                exhausted = True
                skipped.append({'path': r['relative_path'], 'reason': 'byte_budget'})
                continue
            try:
                h = hashlib.sha256()
                with stable_open(root, r) as stream:
                    remaining = size
                    while remaining:
                        data = stream.read(min(CHUNK, remaining))
                        if not data:
                            raise ValueError('Unexpected end of file')
                        read_bytes += len(data)
                        remaining -= len(data)
                        h.update(data)
                digests[(size, h.hexdigest())].append(r)
            except (OSError, ValueError, KeyError) as exc:
                skipped.append({'path': r.get('relative_path', ''), 'reason': str(exc)})
    groups = []
    for (size, digest), rows in sorted(digests.items()):
        if len(rows) < 2:
            continue
        anchor = rows[0]
        verified = [anchor]
        for r in rows[1:]:
            if 2 * size > byte_budget - read_bytes:
                exhausted = True
                skipped.append({'path': r['relative_path'], 'reason': 'verification_budget'})
                continue
            try:
                equal = True
                with stable_open(root, anchor) as a, stable_open(root, r) as b:
                    remaining = size
                    while remaining:
                        block_size = min(CHUNK, remaining)
                        left, right = a.read(block_size), b.read(block_size)
                        read_bytes += len(left) + len(right)
                        if len(left) != block_size or len(right) != block_size or left != right:
                            equal = False
                            break
                        remaining -= block_size
                if equal:
                    verified.append(r)
            except (OSError, ValueError, KeyError) as exc:
                skipped.append({'path': r.get('relative_path', ''), 'reason': str(exc)})
        if len(verified) > 1:
            groups.append({'sha256': digest, 'size_bytes': size,
                           'files': [{'id': r['id'], 'relative_path': r['relative_path']} for r in verified],
                           'logical_duplicate_bytes': size * (len(verified) - 1),
                           'status': 'byte_equal_at_observation', 'deletion_authorised': False})
    return {'groups': groups, 'bytes_read': read_bytes, 'byte_budget': byte_budget,
            'budget_exhausted': exhausted, 'skipped': skipped[:512], 'skipped_count': len(skipped),
            'logical_duplicate_bytes': sum(g['logical_duplicate_bytes'] for g in groups),
            'reclaimable_bytes': None,
            'note': 'Equality is an observation, not a deletion instruction. Files require fresh verification before any later action.'}
