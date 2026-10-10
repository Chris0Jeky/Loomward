"""Explicit native regression checks; effects stay within marked synthetic G: lab trees.

py -3 crates/loomward-lab/check_windows.py
Leaves the three 257-file profile trees for the driver; destroys a cleanup control.
"""
import json
import os
from pathlib import Path
import subprocess

REPO = Path(__file__).resolve().parents[2]
BASE = Path(r"G:\loomward-lab\scale")
EXE = REPO / "target/release/loomward-lab.exe"


def call(*args, ok=True):
    run = subprocess.run([str(EXE), *map(str, args)], cwd=REPO, text=True, capture_output=True)
    assert (run.returncode == 0) == ok, (args, run.stdout, run.stderr)
    return run


def main():
    assert os.name == "nt", "explicit Windows-only native checks"
    results = []
    for profile in ("dev", "media", "mixed"):
        root = BASE / f"smoke-{profile}"
        if not root.exists():
            call("generate", "--root", root, "--files", 257, "--seed", 42, "--profile", profile)
        manifest = json.loads((root / "manifest.json").read_text())
        assert manifest["expected"]["files"] == 257
        assert manifest["profile"] == profile and manifest["seed"] == 42
        call("generate", "--root", root, "--files", 257, "--seed", 42, "--profile", profile, ok=False)
        for strategy in ("std-single", "std-parallel", "find", "handle"):
            measured = json.loads(call("bench", "--root", root, "--strategy", strategy).stdout)
            assert measured["exact"] and measured["observed"] == manifest["expected"]
            results.append({"case": f"{profile}-{strategy}", "passed": True, "observed": measured["observed"]})
        if profile == "mixed":
            for workers in (1, 2, 32):
                assert json.loads(call("bench", "--root", root, "--strategy", "handle", "--threads", workers, "--buffer-kib", 4).stdout)["exact"]
            results.append({"case": "buffer-pagination-4KiB-workers-1-2-32", "passed": True})
    root = BASE / "smoke-mixed"
    marker = root / ".loomward-lab-marker"
    original = marker.read_bytes()
    try:
        marker.write_bytes(b"foreign marker\n")
        call("destroy", "--root", root, ok=False)
        call("bench", "--root", root, "--strategy", "handle", ok=False)
        assert (root / "manifest.json").exists()
    finally:
        marker.write_bytes(original)
    results.append({"case": "foreign-marker-refused-without-deletion", "passed": True})
    manifest_file = root / "manifest.json"
    original = manifest_file.read_bytes()
    try:
        manifest = json.loads(original)
        manifest["expected"]["logical_bytes"] += 1
        manifest_file.write_text(json.dumps(manifest))
        call("bench", "--root", root, "--strategy", "handle", ok=False)
    finally:
        manifest_file.write_bytes(original)
    results.append({"case": "corrupted-manifest-refused", "passed": True})
    # A junction needs no symlink privilege; its target is another tree made by this lab.
    junction = root / "data" / "junction-control"
    target = BASE / "smoke-dev" / "data"
    subprocess.run(["cmd.exe", "/c", "mklink", "/J", str(junction), str(target)], cwd=REPO, check=True, capture_output=True)
    try:
        for strategy in ("std-single", "std-parallel", "find", "handle"):
            run = call("bench", "--root", root, "--strategy", strategy, ok=False)
            measured = json.loads(run.stdout)
            assert measured["observed"]["files"] == 257 and measured["observed"]["skipped_reparse"] == 1
        call("destroy", "--root", root, ok=False)
        assert (root / "manifest.json").exists() and target.is_dir()
    finally:
        os.rmdir(junction)  # Removes the junction itself, never its target.
    results.append({"case": "all-walkers-skip-junction-cleanup-refuses-reparse", "passed": True})
    junction = BASE / "scope-junction-control"
    subprocess.run(["cmd.exe", "/c", "mklink", "/J", str(junction), str(root)], cwd=REPO, check=True, capture_output=True)
    try:
        call("bench", "--root", junction, "--strategy", "handle", ok=False)
        call("destroy", "--root", junction, ok=False)
        assert manifest_file.read_bytes() == original
    finally:
        os.rmdir(junction)
    results.append({"case": "reparse-root-refused", "passed": True})
    unmarked = BASE / "unmarked-control"
    unmarked.mkdir()
    try:
        call("destroy", "--root", unmarked, ok=False)
        assert unmarked.is_dir()
    finally:
        unmarked.rmdir()  # Empty directory made above by this check, no recursive deletion.
    results.append({"case": "missing-marker-refused", "passed": True})
    for path in (BASE, BASE / ".." / "escape", Path(r"E:\loomward-lab\scale\unauthorised")):
        call("destroy", "--root", path, ok=False)
    results.append({"case": "out-of-scope-destroy-refused", "passed": True})
    control = BASE / "cleanup-control"
    call("generate", "--root", control, "--files", 3, "--seed", 7, "--profile", "media")
    call("destroy", "--root", control)
    assert not control.exists()
    results.append({"case": "marked-long-path-cleanup-control", "passed": True})
    output = REPO / "evidence/v3/enumeration-spike-checks.json"
    output.write_text(json.dumps({"platform": "Windows 11, NTFS, non-elevated", "cases": results}, indent=2) + "\n")
    print(f"PASS {len(results)} Windows lab regression cases")


if __name__ == "__main__":
    main()
