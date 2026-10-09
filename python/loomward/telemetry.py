"""Optional, read-only psutil telemetry. Missing data is not reported as zero."""
from __future__ import annotations
import time
from typing import Any


class Telemetry:
    def __init__(self):
        self.previous: dict[tuple[int, float], float] = {}
        self.at: float | None = None

    def snapshot(self, *, max_processes: int = 200) -> dict[str, Any]:
        try:
            if type(max_processes) is not int or not 1 <= max_processes <= 10000:
                raise ValueError('max_processes must be an integer in [1, 10000]')
            import psutil
        except ImportError:
            return {'status': 'unavailable', 'processes': [], 'note': 'Install the optional telemetry extra to inspect processes. No process controls are implemented.'}
        now = time.monotonic()
        duration = now - self.at if self.at is not None else None
        memory = psutil.virtual_memory()
        rows, current = [], {}
        denied = 0
        cores = max(1, psutil.cpu_count() or 1)
        for proc in psutil.process_iter(['pid', 'name', 'create_time', 'memory_info', 'cpu_times']):
            try:
                info = proc.info
                if any(info.get(k) is None for k in ('create_time','memory_info','cpu_times')):
                    denied += 1
                    continue
                key = (info['pid'], info['create_time'])
                cpu = info['cpu_times'].user + info['cpu_times'].system
                current[key] = cpu
                percent = None
                if duration is not None and duration >= 0.1 and key in self.previous:
                    percent = round(max(0.0, min(100.0, (cpu - self.previous[key]) / duration / cores * 100)), 2)
                mi = info['memory_info']
                rows.append({'pid': info['pid'], 'start_time': info['create_time'], 'name': info['name'] or 'Unknown',
                             'resident_bytes': mi.rss, 'private_commit_bytes': getattr(mi, 'private', None),
                             'cpu_percent_of_machine': percent})
            except (psutil.NoSuchProcess, psutil.AccessDenied, psutil.ZombieProcess):
                denied += 1
        self.previous, self.at = current, now
        rows.sort(key=lambda x: (-x['resident_bytes'], x['pid']))
        return {'status': 'observed', 'sample_kind': 'live', 'total_memory_bytes': memory.total,
                'available_memory_bytes': memory.available, 'processes': rows[:max_processes],
                'process_count_observed': len(rows), 'display_truncated': len(rows) > max_processes,
                'denied_or_exited': denied, 'sample_interval_seconds': duration,
                'note': 'Resident memory can include shared pages. CPU needs two samples and is normalised to total logical CPU capacity. No process modifications are supported.'}
