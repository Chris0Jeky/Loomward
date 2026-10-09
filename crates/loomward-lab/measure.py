"""Reproduce the Windows-only scale spike. Writes receipts only in this worktree.

Build first: cargo build -p loomward-lab --release
Run: py -3 crates/loomward-lab/measure.py --sizes 100000 1000000 3000000
Every executable invocation is a fresh process; generation validates with the handle walker,
so neither the first nor repeat passes can honestly be labelled cold-cache.
"""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import time

REPO = Path(__file__).resolve().parents[2]
BASE = Path(r"G:\loomward-lab\scale")
EXE = REPO / "target/release/loomward-lab.exe"
RECEIPT = REPO / "evidence/v3/enumeration-spike.json"


def require_non_elevated():
    from ctypes import wintypes
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    security = ctypes.WinDLL("advapi32", use_last_error=True)
    kernel.GetCurrentProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    security.OpenProcessToken.argtypes = [wintypes.HANDLE, wintypes.DWORD, ctypes.POINTER(wintypes.HANDLE)]
    security.GetTokenInformation.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD, ctypes.POINTER(wintypes.DWORD)]
    token = wintypes.HANDLE()
    elevated, length = wintypes.DWORD(), wintypes.DWORD()
    if not security.OpenProcessToken(kernel.GetCurrentProcess(), 8, ctypes.byref(token)):
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        if not security.GetTokenInformation(token, 20, ctypes.byref(elevated), 4, ctypes.byref(length)):
            raise ctypes.WinError(ctypes.get_last_error())
    finally:
        kernel.CloseHandle(token)
    if elevated.value:
        raise RuntimeError("measure from a non-elevated terminal")


def peak_rss():
    from ctypes import wintypes

    class Counters(ctypes.Structure):
        _fields_ = [("cb", wintypes.DWORD), ("faults", wintypes.DWORD)] + [
            (name, ctypes.c_size_t) for name in (
                "peak_working_set", "working_set", "peak_paged", "paged",
                "peak_nonpaged", "nonpaged", "pagefile", "peak_pagefile")]

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    kernel.GetCurrentProcess.restype = wintypes.HANDLE
    psapi.GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
    data = Counters()
    data.cb = ctypes.sizeof(data)
    if not psapi.GetProcessMemoryInfo(kernel.GetCurrentProcess(), ctypes.byref(data), data.cb):
        raise ctypes.WinError(ctypes.get_last_error())
    return data.peak_working_set


def python_measure(root):
    # Scope is checked by the native lab before this child is dispatched.
    sys.path.insert(0, str(REPO / "python"))
    from loomward.inventory import scan
    expected = json.loads((root / "manifest.json").read_text())["expected"]
    cpu = time.process_time()
    started = time.perf_counter()
    snapshot = scan(root / "data", max_entries=200000, max_depth=64)
    wall = time.perf_counter() - started
    cpu = time.process_time() - cpu
    summary = snapshot["summary"]
    observed = {"files": summary["file_count"], "directories": summary["directory_count"],
                "logical_bytes": summary["logical_bytes"], "skipped_reparse": snapshot["coverage"]["skipped_count"]}
    result = {"strategy": "python-reference", "root": str(root), "threads": 1,
              "cache_label": "warm-uncontrolled", "wall_seconds": wall,
              "files_per_second": observed["files"] / wall, "cpu_seconds": cpu,
              "cpu_percent_one_core": cpu / wall * 100, "peak_rss_bytes": peak_rss(),
              "expected": expected, "observed": observed,
              "exact": observed == expected and snapshot["coverage"]["unqualified_complete"],
              "coverage": snapshot["coverage"]}
    print(json.dumps(result), flush=True)
    if not result["exact"]:
        raise RuntimeError("Python reference totals or coverage failed")


def native(*args):
    run = subprocess.run([str(EXE), *map(str, args)], cwd=REPO, text=True, capture_output=True)
    if run.returncode:
        raise RuntimeError(f"{args}: {run.stderr.strip()} {run.stdout.strip()}")
    return json.loads(run.stdout)


def save(receipt):
    RECEIPT.parent.mkdir(parents=True, exist_ok=True)
    RECEIPT.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sizes", type=int, nargs="+", default=[100000, 1000000])
    parser.add_argument("--threads", type=int, default=8)
    parser.add_argument("--passes", type=int, default=2)
    parser.add_argument("--python-child", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if os.name != "nt":
        parser.error("measurement requires Windows")
    require_non_elevated()
    if args.python_child:
        root = args.python_child
        if root.parent != BASE or root.name != "mixed-100000":
            parser.error("Python comparison is restricted to the marked 100k lab tree")
        native("bench", "--root", root, "--strategy", "handle")
        python_measure(root)
        return
    if args.passes < 1 or args.passes > 5 or not 1 <= args.threads <= 32:
        parser.error("passes must be 1..5 and threads 1..32")
    if any(not 1 <= n <= 3000000 for n in args.sizes):
        parser.error("sizes must be 1..3000000")
    receipt = {"schema_version": 1, "measured_at_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
               "platform": platform.platform(), "python": platform.python_version(),
               "logical_processors": os.cpu_count(), "token_elevated": False, "volume": "G: NTFS, owner-described NVMe",
               "source_base": "1ee166d597b61cf935a84499351009f0e316857a",
               "build": "cargo build -p loomward-lab --release --offline",
               "executable_sha256": hashlib.sha256(EXE.read_bytes()).hexdigest(),
               "source_sha256": {str(p.relative_to(REPO)): hashlib.sha256(p.read_bytes()).hexdigest()
                                 for p in sorted((REPO / "crates/loomward-lab/src").glob("*.rs"))},
               "cold_cache": "NOT measured: generation and validation populate caches; no eviction/reboot/elevation",
               "complete": False, "trees": [], "runs": []}
    save(receipt)
    strategies = ["std-single", "std-parallel", "find", "handle"]
    for count in args.sizes:
        root = BASE / f"mixed-{count}"
        reused = root.exists()
        if reused:
            # Refuse any tree whose marker or manifest the native executable cannot validate.
            native("bench", "--root", root, "--strategy", "handle")
            manifest = json.loads((root / "manifest.json").read_text())
            if manifest["seed"] != 42 or manifest["profile"] != "mixed" or manifest["expected"]["files"] != count:
                raise RuntimeError("existing tree is not this experiment's deterministic plan")
        else:
            print(f"generating {count:,} sparse files with {args.threads} workers", flush=True)
            manifest = native("generate", "--root", root, "--files", count,
                              "--seed", 42, "--profile", "mixed", "--threads", args.threads)
        receipt["trees"].append({"root": str(root), **manifest})
        save(receipt)
        for repetition in range(args.passes):
            order = strategies if repetition % 2 == 0 else list(reversed(strategies))
            for strategy in order:
                result = native("bench", "--root", root, "--strategy", strategy,
                                "--threads", args.threads, "--buffer-kib", 64,
                                "--cache-label", "post-generation-uncontrolled" if repetition == 0 and not reused else "warm-uncontrolled")
                result["pass"] = repetition + 1
                receipt["runs"].append(result)
                save(receipt)
                print(f"{count:,} {strategy:12} pass {repetition + 1}: {result['wall_seconds']:.3f}s, "
                      f"{result['files_per_second']:,.0f} files/s, RSS {result['peak_rss_bytes']/1048576:.2f} MiB, exact={result['exact']}", flush=True)
        if count == 100000:
            run = subprocess.run([sys.executable, __file__, "--python-child", str(root)], cwd=REPO, text=True, capture_output=True, check=True)
            result = json.loads(run.stdout)
            receipt["runs"].append(result)
            save(receipt)
            print(f"100,000 Python reference: {result['wall_seconds']:.3f}s, RSS {result['peak_rss_bytes']/1048576:.2f} MiB, exact={result['exact']}", flush=True)
    # A small parameter sweep exercises the changed enumeration seam without repeating the
    # expensive per-file stat baselines. Keep every raw trial, not just the fastest one.
    if 1000000 in args.sizes:
        root = BASE / "mixed-1000000"
        receipt["tuning_runs"] = []
        for workers, buffer in [(n, 64) for n in (1, 2, 4, 8, 16)] + [(8, n) for n in (4, 16, 256)]:
            for trial in range(3):
                result = native("bench", "--root", root, "--strategy", "handle",
                                "--threads", workers, "--buffer-kib", buffer, "--cache-label", "warm-uncontrolled")
                result["trial"] = trial + 1
                receipt["tuning_runs"].append(result)
                save(receipt)
            print(f"handle tuning: workers={workers}, buffer={buffer} KiB done", flush=True)
    if 3000000 in args.sizes:
        root = BASE / "mixed-3000000"
        receipt["tuning_3m"] = []
        for trial in range(3):
            for buffer in (16, 64):
                result = native("bench", "--root", root, "--strategy", "handle",
                                "--threads", 8, "--buffer-kib", buffer, "--cache-label", "warm-uncontrolled")
                result["trial"] = trial + 1
                receipt["tuning_3m"].append(result)
                save(receipt)
    receipt["complete"] = True
    save(receipt)


if __name__ == "__main__":
    main()
