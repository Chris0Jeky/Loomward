"""Transactional local feedback and an unkeyed, tamper-evident audit chain."""
from __future__ import annotations
from contextlib import contextmanager
import hashlib
import json
import os
import sqlite3
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable
from .inventory import checked_root
from .learning import validate_features, validate_labels

APPLICATION_ID = 0x4C574452
GENESIS = '0' * 64


def canonical(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True, allow_nan=False)


class Store:
    def __init__(self, path: str | Path):
        self.path = Path(path).absolute()
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        checked_root(self.path.parent)
        if self.path.exists() and (self.path.is_symlink() or self.path.stat().st_nlink > 1):
            raise ValueError('State database must not be a link')
        with self._connect() as c:
            app = c.execute('PRAGMA application_id').fetchone()[0]
            existing = c.execute("SELECT count(*) FROM sqlite_master WHERE type='table'").fetchone()[0]
            if app not in (0, APPLICATION_ID) or (app == 0 and existing):
                raise ValueError('Refusing to modify a database owned by another application')
            if c.execute('PRAGMA user_version').fetchone()[0] not in (0, 1):
                raise ValueError('Unsupported state database version')
            c.execute(f'PRAGMA application_id={APPLICATION_ID}')
            c.execute('PRAGMA user_version=1')
            c.execute('PRAGMA journal_mode=WAL')
            c.executescript('''
                CREATE TABLE IF NOT EXISTS feedback (
                    event_id TEXT PRIMARY KEY, scope TEXT NOT NULL, item_id TEXT NOT NULL,
                    source TEXT NOT NULL, revision INTEGER NOT NULL, payload TEXT NOT NULL,
                    UNIQUE(scope,item_id,source,revision));
                CREATE INDEX IF NOT EXISTS feedback_scope ON feedback(scope);
                CREATE TABLE IF NOT EXISTS audit (
                    seq INTEGER PRIMARY KEY AUTOINCREMENT, payload TEXT NOT NULL,
                    prev_hash TEXT NOT NULL, hash TEXT NOT NULL);
            ''')
        if os.name != 'nt':
            os.chmod(self.path, 0o600)

    @contextmanager
    def _connect(self):
        c = sqlite3.connect(self.path, timeout=5)
        try:
            c.execute('PRAGMA synchronous=FULL')
            with c:
                yield c
        finally:
            c.close()

    def feedback(self, scope: str, item_id: str, features: dict[str, Any], label: str,
                 *, source: str = 'human', event_id: str | None = None, retracted: bool = False,
                 validate_batch: Callable[[list[dict[str, Any]]], Any] | None = None) -> dict[str, Any]:
        if not isinstance(scope, str) or not 1 <= len(scope) <= 128 or not isinstance(item_id, str) or not 1 <= len(item_id) <= 128:
            raise ValueError('Invalid scope or item ID')
        if source not in ('human', 'teacher') or type(retracted) is not bool:
            raise ValueError('Invalid provenance')
        validate_labels([label])
        features = validate_features(features)
        event_id = event_id or str(uuid.uuid4())
        if not isinstance(event_id, str) or not 1 <= len(event_id) <= 128:
            raise ValueError('Invalid event ID')
        base = {'scope': scope, 'item_id': item_id, 'features': features, 'label': label,
                'source': source, 'event_id': event_id, 'retracted': retracted}
        with self._connect() as c:
            c.execute('BEGIN IMMEDIATE')
            previous = c.execute('SELECT payload FROM feedback WHERE event_id=?', (event_id,)).fetchone()
            if previous:
                value = json.loads(previous[0])
                if any(value[k] != v for k, v in base.items()):
                    raise ValueError('Event ID already exists with different contents')
                return value
            revision = c.execute('SELECT COALESCE(MAX(revision),0)+1 FROM feedback WHERE scope=? AND item_id=? AND source=?',
                                 (scope, item_id, source)).fetchone()[0]
            value = {**base, 'revision': revision, 'recorded_at': datetime.now(timezone.utc).isoformat()}
            rows = c.execute('SELECT payload FROM feedback WHERE scope=? ORDER BY rowid LIMIT 10001', (scope,)).fetchall()
            if len(rows) >= 10000:
                raise ValueError('Feedback event budget exceeded; no event was committed')
            if validate_batch is not None:
                validate_batch([json.loads(row[0]) for row in rows] + [value])
            text = canonical(value)
            c.execute('INSERT INTO feedback VALUES (?,?,?,?,?,?)', (event_id, scope, item_id, source, revision, text))
            prev = c.execute('SELECT hash FROM audit ORDER BY seq DESC LIMIT 1').fetchone()
            prev_hash = prev[0] if prev else GENESIS
            payload = canonical({'kind': 'feedback.recorded', 'event': value})
            digest = hashlib.sha256((prev_hash + '\n' + payload).encode()).hexdigest()
            c.execute('INSERT INTO audit(payload,prev_hash,hash) VALUES (?,?,?)', (payload, prev_hash, digest))
            return value

    def events(self, scope: str) -> list[dict[str, Any]]:
        with self._connect() as c:
            rows = c.execute('SELECT payload FROM feedback WHERE scope=? ORDER BY rowid LIMIT 10001', (scope,)).fetchall()
        if len(rows) > 10000:
            raise ValueError('Prototype training event budget exceeded; export and design a retention window explicitly')
        return [json.loads(row[0]) for row in rows]

    def audit(self, limit: int = 200) -> dict[str, Any]:
        with self._connect() as c:
            total = c.execute('SELECT COUNT(*) FROM audit').fetchone()[0]
            rows = c.execute('SELECT seq,payload,prev_hash,hash FROM audit ORDER BY seq DESC LIMIT ?', (limit,)).fetchall()
        return {'total': total, 'has_more': total > len(rows),
                'events': [{'seq': row[0], 'payload': json.loads(row[1]), 'prev_hash': row[2], 'hash': row[3]} for row in rows]}

    def verify_audit(self) -> bool:
        previous, next_seq = GENESIS, 1
        with self._connect() as c:
            for seq, payload, prev_hash, digest in c.execute('SELECT seq,payload,prev_hash,hash FROM audit ORDER BY seq'):
                if seq != next_seq or prev_hash != previous or hashlib.sha256((previous + '\n' + payload).encode()).hexdigest() != digest:
                    return False
                previous, next_seq = digest, next_seq + 1
        return True
