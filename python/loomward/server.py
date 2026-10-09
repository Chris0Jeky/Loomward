"""Source-checkout loopback UI. No arbitrary filesystem paths or shell commands in IPC."""
from __future__ import annotations
import hmac
import json
import secrets
import shutil
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from .inventory import scan, features_for
from .duplicates import find_duplicates
from .learning import Student, DEFAULT_LABELS
from .planner import plan_tiers
from .planner_v2 import plan_tiers_v2
from .scheduler import admit_workloads
from .store import Store
from .telemetry import Telemetry
from .teacher import strict_json

PROJECT = Path(__file__).resolve().parents[2]
ASSETS = {'/expansion.js': ('expansion.js', 'text/javascript; charset=utf-8'), '/': ('index.html', 'text/html; charset=utf-8'), '/app.js': ('app.js', 'text/javascript; charset=utf-8'),
          '/styles.css': ('styles.css', 'text/css; charset=utf-8'), '/demo-data.js': ('demo-data.js', 'text/javascript; charset=utf-8')}
MAX_BODY = 65536
DRAIN_LIMIT = 1024 * 1024


class App:
    def __init__(self, state_path: str | Path, *, root: str | Path | None = None, demo: bool = False,
                 allow_processes: bool = False, max_entries: int = 50_000):
        if demo == (root is not None):
            raise ValueError('Select exactly one of demo mode or an explicit scan root')
        self.lock = threading.RLock()
        self.store = Store(state_path)
        self.demo = demo
        self.root = root
        self.allow_processes = allow_processes
        self.max_entries = max_entries
        self.telemetry = Telemetry()
        self.labels = list(DEFAULT_LABELS)
        self.seed_events: list[dict] = []
        if demo:
            data = json.loads((PROJECT / 'fixtures' / 'demo.json').read_text(encoding='utf-8'))
            self.inventory = data['inventory']
            self.volumes = data['volumes']
            self.scenario = data['scenario']
            self.processes = data['processes']
            self.seed_events = data['seed_events']
            self.duplicate_examples = data['duplicate_examples']
            self.scope = 'synthetic-demo'
        else:
            self.inventory = scan(root, max_entries=max_entries)
            self.scope = 'personal-' + self.inventory['root_key']
            usage = shutil.disk_usage(root)
            self.volumes = [{'id': 'selected-root-volume', 'name': str(root), 'capacity_bytes': usage.total,
                             'free_bytes': usage.free, 'tier': None, 'reserve_bytes': None,
                             'note': 'OS capacity observation. Speed tier is unknown, not inferred from a drive letter.'}]
            self.scenario = None
            self.processes = {'status': 'not_requested', 'processes': [], 'note': 'Process inspection requires the --processes start option.'}
            self.duplicate_examples = []
        self.duplicates = None
        self.last_plan = None
        self._fit()

    def _fit(self):
        events = self.seed_events + self.store.events(self.scope)
        self.model = Student(self.labels).fit(events)
        self.predictions = {f['id']: self.model.predict(features_for(f)) for f in self.inventory['files'][:2000]
                            if 'sensitive' not in f.get('flags', [])}

    def state(self) -> dict[str, Any]:
        with self.lock:
            return {'inventory': self.inventory, 'volumes': self.volumes, 'scenario': self.scenario,
                    'processes': self.processes, 'duplicates': self.duplicates, 'last_plan': self.last_plan,
                    'labels': self.labels, 'predictions': self.predictions,
                    'learning': {'training_count': self.model.training_count, 'human_support': self.model.human_support,
                                 'model_id': self.model.model_id, 'calibrated': False,
                                 'prediction_limit': 2000, 'synthetic_seed_count': len(self.seed_events)},
                    'audit': self.scoped_audit(), 'capabilities': {'file_mutation': False, 'process_mutation': False,
                        'process_inspection': self.demo or self.allow_processes, 'teacher_network': False},
                    'runtime': 'python_reference', 'mode': 'demo' if self.demo else 'observed'}

    def scoped_audit(self) -> dict[str, Any]:
        # A profile view is not a cryptographic audit verification. The complete chain
        # remains in the database and Store.verify_audit verifies it independently.
        events = self.store.events(self.scope)
        return {'total': len(events), 'has_more': len(events) > 200,
                'events': [{'payload': {'event': e}} for e in reversed(events[-200:])]}

    def feedback_export(self) -> dict[str, Any]:
        with self.lock:
            return {'schema_version': 1, 'mode': 'demo' if self.demo else 'observed',
                    'scope': self.scope, 'events': self.store.events(self.scope), 'complete': True,
                    'note': 'User feedback only; synthetic seed events are not included.'}

    def post(self, path: str, body: dict[str, Any]) -> dict[str, Any]:
        with self.lock:
            if path == '/api/label':
                if set(body) - {'item_id','label','event_id','retracted'}:
                    raise ValueError('Unknown label fields')
                item = next((f for f in self.inventory['files'] if f['id'] == body.get('item_id')), None)
                if item is None or 'sensitive' in item.get('flags', []):
                    raise ValueError('Unknown or excluded item')
                if body.get('label') not in self.labels:
                    raise ValueError('Label is outside the taxonomy')
                event = self.store.feedback(self.scope, item['id'], features_for(item), body['label'],
                                            event_id=body.get('event_id'), retracted=body.get('retracted', False),
                                            validate_batch=lambda events: Student(self.labels).fit(self.seed_events + events))
                self._fit()
                return {'event': event, 'learning': {'model_id': self.model.model_id, 'training_count': self.model.training_count},
                        'filesystem_changed': False}
            if path == '/api/train':
                if body:
                    raise ValueError('Training accepts no injected examples through this endpoint')
                self._fit()
                return {'model_id': self.model.model_id, 'training_count': self.model.training_count, 'calibrated': False}
            if path == '/api/rescan':
                if body:
                    raise ValueError('Scan roots can only be selected when starting the application')
                if not self.demo:
                    self.inventory = scan(self.root, max_entries=self.max_entries)
                    self.duplicates = None
                    self._fit()
                return {'inventory': self.inventory}
            if path == '/api/duplicates':
                if set(body) - {'confirm_content_read', 'byte_budget'} or body.get('confirm_content_read') is not True:
                    raise ValueError('Explicit content-read consent is required')
                if self.demo:
                    self.duplicates = {'groups': self.duplicate_examples, 'mode': 'demo', 'bytes_read': 0,
                                       'logical_duplicate_bytes': sum(x['logical_duplicate_bytes'] for x in self.duplicate_examples),
                                       'reclaimable_bytes': None, 'note': 'Synthetic duplicate example; no files were read.'}
                else:
                    self.duplicates = find_duplicates(self.root, self.inventory, byte_budget=body.get('byte_budget', 256 * 1024**2))
                return self.duplicates
            if path == '/api/plan-v2':
                self.last_plan = plan_tiers_v2(body, node_budget=5000)
                return self.last_plan
            if path == '/api/schedule':
                return admit_workloads(body)
            if path == '/api/plan':
                self.last_plan = plan_tiers(body)
                return self.last_plan
            if path == '/api/processes':
                if body:
                    raise ValueError('Process inspection accepts no process control fields')
                if not self.demo:
                    if not self.allow_processes:
                        raise ValueError('Process inspection was not enabled at startup')
                    self.processes = self.telemetry.snapshot()
                return self.processes
            raise LookupError('Endpoint does not exist; file and process mutations are not implemented')


class Server(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = False

    def __init__(self, address: tuple[str, int], app: App, token: str | None = None):
        if address[0] != '127.0.0.1':
            raise ValueError('Reference server binds only to 127.0.0.1')
        self.app = app
        self.token = token or secrets.token_urlsafe(32)
        super().__init__(address, Handler)

    @property
    def origin(self):
        return f'http://127.0.0.1:{self.server_port}'


class Handler(BaseHTTPRequestHandler):
    server: Server

    def setup(self):
        super().setup()
        self.connection.settimeout(10)

    def log_message(self, fmt, *args):
        # Do not put private paths, tokens or query strings in access logs.
        return

    def _send(self, status: int, data: bytes, content_type: str = 'application/json; charset=utf-8'):
        self.send_response(status)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(data)))
        self.send_header('Cache-Control', 'no-store')
        self.send_header('X-Content-Type-Options', 'nosniff')
        self.send_header('Referrer-Policy', 'no-referrer')
        self.send_header('Cross-Origin-Resource-Policy', 'same-origin')
        self.send_header('Cross-Origin-Opener-Policy', 'same-origin')
        self.send_header('Content-Security-Policy', "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'")
        self.end_headers()
        self.wfile.write(data)

    def _json(self, status: int, value: Any):
        self._send(status, json.dumps(value, ensure_ascii=True, allow_nan=False).encode())

    def _authorised(self, api: bool) -> bool:
        if self.headers.get('Host') != f'127.0.0.1:{self.server.server_port}':
            self._json(403, {'error': 'Unexpected Host'}); return False
        origin = self.headers.get('Origin')
        if origin is not None and origin != self.server.origin:
            self._json(403, {'error': 'Cross-origin requests are forbidden'}); return False
        if api and not hmac.compare_digest(self.headers.get('X-Loomward-Token', '').encode('utf-8'), self.server.token.encode('utf-8')):
            self._json(403, {'error': 'A valid local session token is required'}); return False
        return True

    def do_GET(self):
        if not self._authorised(self.path.startswith('/api/')):
            return
        if self.path == '/api/state':
            self._json(200, self.server.app.state()); return
        if self.path == '/api/audit':
            self._json(200, self.server.app.scoped_audit()); return
        if self.path == '/api/feedback':
            self._json(200, self.server.app.feedback_export()); return
        asset = ASSETS.get(self.path)
        if asset is None:
            self._json(404, {'error': 'Not found'}); return
        path = PROJECT / 'ui' / asset[0]
        if not path.is_file():
            self._json(503, {'error': 'UI assets are missing; run from the complete source checkout'}); return
        self._send(200, path.read_bytes(), asset[1])

    def do_POST(self):
        chunked = bool(self.headers.get('Transfer-Encoding'))
        try:
            length = int(self.headers.get('Content-Length', '-1'))
        except ValueError:
            length = -1
        # Read or drain a bounded body before any reply: Windows resets a socket closed with unread
        # request bytes, which can destroy an early error reply before the client reads it.
        raw = self.rfile.read(length) if not chunked and 0 <= length <= DRAIN_LIMIT else b''
        if not self._authorised(True):
            self.close_connection = True
            return
        if chunked:
            self.close_connection = True; self._json(400, {'error': 'Chunked request bodies are not supported'}); return
        if not 0 <= length <= MAX_BODY:
            self.close_connection = True; self._json(413, {'error': 'Request body exceeds the 64 KiB limit or has no valid length'}); return
        if self.headers.get('Content-Type', '').split(';')[0].lower() != 'application/json':
            self.close_connection = True; self._json(415, {'error': 'Use application/json'}); return
        try:
            body = strict_json(raw.decode('utf-8'))
            if not isinstance(body, dict):
                raise ValueError('JSON object required')
            result = self.server.app.post(self.path, body)
            self._json(200, result)
        except LookupError as exc:
            self._json(404, {'error': str(exc)})
        except (ValueError, UnicodeError, OSError, TypeError) as exc:
            self._json(400, {'error': str(exc)[:1000]})
        except Exception:
            self._json(500, {'error': 'Internal reference-runtime failure; no file or process action was executed'})
