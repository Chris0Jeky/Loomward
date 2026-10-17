"""Apply docs/45 row 7's predeclared rule to a private soak JSONL (stdlib only)."""
import argparse
import json
import math
import statistics
from collections import defaultdict

STAGES = ("grant", "scan", "tree.slice", "search.query", "watch", "refresh", "revoke", "forget", "restart")
MIB = 1024 * 1024


def evaluate(records, smoke=False):
    errors = set()
    start = end = None
    restarts = []
    latencies = defaultdict(list)
    peaks = defaultdict(int)
    previous = 0
    cycle = 1
    stage_index = 0
    samples = 0
    for record in records:
        kind = record["kind"]
        if end is not None:
            errors.add("records after end")
        if kind == "start":
            if start is not None or samples:
                errors.add("duplicate or misplaced start")
            start = record
            if start["schema_version"] != 1:
                errors.add("unsupported schema")
        elif kind == "end":
            end = record
        elif kind == "sample":
            if start is None:
                raise ValueError("sample before start")
            samples += 1
            operation = record["operation"]
            if record["cycle"] != cycle or operation != STAGES[stage_index]:
                errors.add("missing, duplicate or unordered operation")
            stage_index += 1
            if stage_index == len(STAGES):
                cycle += 1
                stage_index = 0
            elapsed = record["elapsed_s"]
            if not math.isfinite(elapsed) or elapsed < previous or elapsed - previous > 60:
                errors.add("sample continuity gap or invalid timestamp")
            previous = elapsed
            if record["error"] is not None:
                errors.add("operation error: " + str(record["error"]))
                continue
            for key in ("handles", "rss_bytes", "private_bytes", "latency_ms"):
                if not isinstance(record[key], (int, float)) or not math.isfinite(record[key]) or record[key] < 0:
                    errors.add("invalid metric: " + key)
                elif key != "latency_ms" and record[key] == 0:
                    errors.add("missing process measurement: " + key)
            sizes = record["sizes"]
            wal = sizes["catalog.db-wal"] + sizes["state.db-wal"]
            db = sizes["catalog.db"] + sizes["state.db"]
            if any(not isinstance(v, int) or v < 0 for v in sizes.values()):
                errors.add("invalid database size")
            if wal > 64 * MIB or db > 64 * MIB:
                errors.add("WAL or live DB exceeds 64 MiB")
            for key, value in (("wal_bytes", wal), ("db_bytes", db),
                               *((key, record[key]) for key in ("handles", "rss_bytes", "private_bytes"))):
                peaks[key] = max(peaks[key], value)
            rows = record["rows"]
            expected = start["manifest"]["expected"]
            if operation in ("scan", "refresh"):
                if (rows["file"], rows["dir"], rows["root"]) != (expected["files"], expected["directories"] + 1, 1):
                    errors.add("scan rows disagree with manifest")
                if rows["stage_entry"]:
                    errors.add("scan retained staged rows")
                if record["logical_bytes"] != expected["logical_bytes"]:
                    errors.add("scan bytes disagree with manifest")
            if operation in ("forget", "restart") and any(rows.values()):
                errors.add("forget or restart retained rows")
            latencies[operation].append(record["latency_ms"])
            if operation == "restart":
                restarts.append(record)
        else:
            errors.add("unknown record kind")
    minimum = 900 if smoke else 72 * 3600
    warmup, window = (60, 180) if smoke else (300, 3600)
    if start is None or end is None:
        errors.add("missing start or clean end")
    duration = end["elapsed_s"] if end else previous
    if end:
        if (not end["success"] or end["error"] is not None or stage_index
                or end["cycles"] != cycle - 1 or duration < previous or duration - previous > 60):
            errors.add("incomplete lifecycle or unsuccessful end")
    if not math.isfinite(duration) or duration < minimum:
        errors.add(f"duration below {minimum} seconds")
    if start and (not math.isfinite(start["requested_seconds"]) or start["requested_seconds"] < minimum or duration < start["requested_seconds"]):
        errors.add("requested run incomplete or too short")
    first = [r for r in restarts if warmup <= r["elapsed_s"] < warmup + window]
    last = [r for r in restarts if duration - window <= r["elapsed_s"] <= duration]
    growth = {}
    if duration - window < warmup + window or min(len(first), len(last)) < 12:
        errors.add("insufficient independent comparison windows")
    else:
        for key, absolute, relative in (("handles", 32, .1), ("private_bytes", 32 * MIB, .1), ("rss_bytes", 64 * MIB, .2)):
            before = statistics.median(r[key] for r in first)
            after = statistics.median(r[key] for r in last)
            bound = max(absolute, before * relative)
            growth[key] = {"first_median": before, "last_median": after, "growth": after - before, "allowed": bound}
            if after - before > bound:
                errors.add(key + " growth exceeds bound")
    return {"status": "FAIL" if errors else "SMOKE PASS" if smoke else "PASS",
            "duration_seconds": duration, "cycles": cycle - 1, "samples": samples,
            "comparison_cycles": [len(first), len(last)], "growth": growth, "peaks": dict(peaks),
            "latency_ms": {k: {"median": statistics.median(v), "max": max(v)} for k, v in latencies.items()},
            "errors": sorted(errors)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input")
    parser.add_argument("--smoke", action="store_true", help="15-minute check; never qualifies row 7")
    args = parser.parse_args()
    try:
        with open(args.input, encoding="utf-8") as source:
            result = evaluate((json.loads(line) for line in source), args.smoke)
    except (OSError, ValueError, KeyError, TypeError, IndexError) as error:
        result = {"status": "FAIL", "errors": [f"invalid or incomplete JSONL: {error}"]}
    print(json.dumps(result, indent=2))
    return 1 if result["status"] == "FAIL" else 0


if __name__ == "__main__":
    raise SystemExit(main())
