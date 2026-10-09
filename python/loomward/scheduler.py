"""Resource admission simulation and an ephemeral cooperative lease reference.

Budget values are supplied policy/estimate inputs, not measured OS guarantees.
No subprocess is started, paused, prioritised, terminated or inspected here.
"""
from __future__ import annotations
import copy
import hashlib
import hmac
import json
import math
import secrets
import threading
import time
from typing import Callable
from .catalog import text

DIMENSIONS = ('cpu_slots', 'memory_mib', 'gpu_mib', 'io_mib_s')
MAX_VALUE = 1_000_000_000


def number(value, lo=0, hi=MAX_VALUE):
    if type(value) is not int or not lo <= value <= hi:
        raise ValueError('Resource value must be an integer within its limit')
    return value


def vector(value: dict, *, unknown=False) -> dict:
    if not isinstance(value, dict) or set(value) - set(DIMENSIONS):
        raise ValueError('Unknown resource dimensions')
    result = {}
    for dimension in DIMENSIONS:
        v = value.get(dimension, 0)
        result[dimension] = None if unknown and v is None else number(v)
    return result


def _remaining(capacity: dict, reserved: dict) -> dict:
    return {d: None if capacity[d] is None else max(0, capacity[d] - reserved[d]) for d in DIMENSIONS}


def _blocking(remaining: dict, demand: dict) -> list[str]:
    reasons = []
    for d in DIMENSIONS:
        if demand[d] and remaining[d] is None:
            reasons.append('unknown_' + d)
        elif remaining[d] is not None and demand[d] > remaining[d]:
            reasons.append('insufficient_' + d)
    return reasons


def admit_workloads(scenario: dict) -> dict:
    if not isinstance(scenario, dict) or set(scenario) - {'capacity', 'reserved', 'jobs', 'telemetry_age_seconds'}:
        raise ValueError('Unsupported admission scenario')
    capacity = vector(scenario.get('capacity'), unknown=True)
    reserved = vector(scenario.get('reserved', {}))
    age = number(scenario.get('telemetry_age_seconds', 0), hi=86400)
    jobs = scenario.get('jobs')
    if not isinstance(jobs, list) or len(jobs) > 1000:
        raise ValueError('Expected at most 1000 jobs')
    prepared = []; seen = set()
    for job in jobs:
        if not isinstance(job, dict) or set(job) - {'id', 'demand', 'priority', 'waited_seconds'}:
            raise ValueError('Unsupported workload record')
        ident = text(job.get('id'), 'job ID', 128)
        if ident in seen:
            raise ValueError('Duplicate workload ID')
        seen.add(ident)
        request = vector(job.get('demand'))
        priority = number(job.get('priority', 0), hi=10)
        waited = number(job.get('waited_seconds', 0), hi=604800)
        prepared.append({'job_id': ident, 'demand': request,
                         'effective_priority': min(10, priority + waited // 300), 'waited_seconds': waited})
    prepared.sort(key=lambda j: (-j['effective_priority'], -j['waited_seconds'], j['job_id']))
    remaining = _remaining(capacity, reserved)
    accepted = []; deferred = []
    for job in prepared:
        reasons = ['stale_capacity_observation'] if age > 30 else _blocking(remaining, job['demand'])
        if reasons:
            deferred.append({'job_id': job['job_id'], 'reasons': reasons})
            continue
        for d in DIMENSIONS:
            if remaining[d] is not None:
                remaining[d] -= job['demand'][d]
        accepted.append(dict(job, status='simulated_admission', execution_authority=False))
    return {'mode': 'simulation', 'algorithm': 'priority_age_vector_admission_v1',
            'capacity': capacity, 'reserved': reserved, 'remaining': remaining,
            'overcommitted': [d for d in DIMENSIONS if capacity[d] is not None and reserved[d] > capacity[d]],
            'accepted': accepted, 'deferred': deferred, 'processes_changed': False,
            'assumption': 'Supplied incremental resource estimates; reservations counted once. No OS enforcement or fairness guarantee.'}


class LeaseBroker:
    """Single-process, lock-protected reservations for cooperating reference callers.

    Self-declared owners are NOT authenticated identities. The token is a bearer
    release capability, not authority over a real workload. State deliberately
    does not survive a restart; do not use this as a production resource daemon.
    """
    def __init__(self, capacity: dict, *, clock: Callable[[], float] = time.monotonic):
        self._capacity = vector(capacity, unknown=True)
        self._clock = clock
        self._last_now = -1.0
        self._lock = threading.RLock()
        self._requests: dict[tuple[str, str], dict] = {}
        self._leases: dict[str, dict] = {}
        self.epoch = secrets.token_hex(16)

    @property
    def capacity(self) -> dict:
        return dict(self._capacity)

    def _now(self) -> float:
        now = self._clock()
        if type(now) not in (int, float) or not math.isfinite(now) or now < self._last_now or now < 0:
            raise ValueError('Lease clock is invalid or moved backwards')
        self._last_now = now
        for lease in self._leases.values():
            if lease['status'] == 'granted' and now >= lease['expires_at']:
                lease['status'] = 'expired'
        return now

    def _reserved(self):
        return {d: sum(x['demand'][d] for x in self._leases.values() if x['status'] == 'granted') for d in DIMENSIONS}

    @staticmethod
    def _receipt(record: dict) -> dict:
        return copy.deepcopy({k: v for k, v in record.items() if k != 'fingerprint'})

    def acquire(self, owner: str, request_id: str, demand: dict, *, ttl_seconds: int = 30) -> dict:
        text(owner, 'owner', 128); text(request_id, 'request ID', 128)
        request = vector(demand); number(ttl_seconds, lo=5, hi=3600)
        fingerprint = hashlib.sha256(json.dumps([request, ttl_seconds], sort_keys=True).encode()).hexdigest()
        with self._lock:
            now = self._now()
            key = (owner, request_id)
            prior = self._requests.get(key)
            if prior is not None:
                if prior['fingerprint'] != fingerprint:
                    raise ValueError('Idempotency key reused for a different request')
                return self._receipt(prior)
            if len(self._requests) >= 10_000:
                raise ValueError('Ephemeral lease history budget exhausted; start a new broker epoch')
            reasons = _blocking(_remaining(self.capacity, self._reserved()), request)
            record = {'owner': owner, 'request_id': request_id, 'demand': request,
                      'fingerprint': fingerprint, 'epoch': self.epoch,
                      'status': 'deferred' if reasons else 'granted',
                      'reasons': reasons, 'execution_authority': False}
            if not reasons:
                lease_id = secrets.token_hex(16)
                record.update(lease_id=lease_id, token=secrets.token_urlsafe(32), expires_at=now + ttl_seconds)
                self._leases[lease_id] = record
            self._requests[key] = record
            return self._receipt(record)

    def release(self, lease_id: str, owner: str, token: str) -> bool:
        text(lease_id, 'lease ID', 128); text(owner, 'owner', 128); text(token, 'token', 128)
        with self._lock:
            self._now()
            lease = self._leases.get(lease_id)
            if lease is None or lease['owner'] != owner or not hmac.compare_digest(lease['token'].encode('utf-8'), token.encode('utf-8')):
                raise PermissionError('Lease release capability is invalid')
            if lease['status'] == 'expired':
                return False
            lease['status'] = 'released'
            return True

    def snapshot(self) -> dict:
        with self._lock:
            self._now()
            reserved = self._reserved()
            return {'epoch': self.epoch, 'capacity': dict(self.capacity), 'reserved': reserved,
                    'remaining': _remaining(self.capacity, reserved),
                    'active_leases': sum(x['status'] == 'granted' for x in self._leases.values()),
                    'request_count': len(self._requests), 'durable': False, 'processes_changed': False}
