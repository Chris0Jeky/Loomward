"""Explicit real-root measurements; paths stay in an external private plan/log.

Build the release binary first. Supply --plan (JSON label/path pairs), --root-log,
and optionally --synthetic for the identical marked G:/E: million-file comparison.
No snapshots or per-entry diagnostics are persisted. No cache eviction is attempted.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import time

from measure import EXE, REPO, peak_rss, require_non_elevated

RECEIPT = REPO / "evidence/v3/real-folder-stress.json"


def save(receipt):
    RECEIPT.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")


def native(*args):
    run = subprocess.run([str(EXE), *map(str, args)], cwd=REPO, capture_output=True, text=True)
    if not run.stdout.strip():
        raise RuntimeError(f"native command failed before aggregate output (exit {run.returncode})")
    result = json.loads(run.stdout)
    if run.returncode and not (result.get("agreement") is False):
        raise RuntimeError(f"native command failed (exit {run.returncode})")
    return result


def python_child(root, label, root_log):
    text = str(root)
    if (len(text) < 4 or not text[0].isascii() or not text[0].isalpha()
            or text[1:3] != ":\\"):
        raise ValueError("real root must be an absolute local path below a volume root")
    parts = text[3:].split("\\")
    if any(not part or part in (".", "..") or part.endswith((".", " "))
           or any(char in part for char in ":/*?\0") for part in parts) or any(
            part.lower() in ("users", "appdata", "browser", "chrome", "chromium", "firefox",
                             "edge", "credentials", "keys", ".ssh", ".aws", ".azure", ".gnupg")
            for part in parts):
        raise ValueError("profile, credential, browser, or ambiguous real root refused")
    root = Path(root)
    if any(path.is_symlink() or path.is_junction() for path in (root, *root.parents)):
        raise ValueError("reparse real root refused")
    if not root_log.is_absolute():
        raise ValueError("root-log must be absolute and outside the scan root")
    try:
        resolved_root = root.resolve(strict=True)
        parent = root_log.parent.resolve(strict=True)
    except OSError as error:
        raise ValueError("root and root-log parent must resolve") from error
    root_key = Path(str(resolved_root).removeprefix("\\\\?\\"))
    parent_key = Path(str(parent).removeprefix("\\\\?\\"))
    if not resolved_root.is_dir() or not parent.is_dir() or parent_key.is_relative_to(root_key):
        raise ValueError("root-log must be absolute and outside the scan root")
    root_log = parent / root_log.name
    with root_log.open("a", encoding="utf-8") as log:
        log.write(f"{time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}\t{root}\towner-authorised Python metadata comparison; {label}\n")
        log.flush()
        os.fsync(log.fileno())
    sys.path.insert(0, str(REPO / "python"))
    from loomward.inventory import scan
    cpu = time.process_time()
    started = time.perf_counter()
    snapshot = scan(root, max_entries=200000, max_depth=128, exclude_names=frozenset())
    wall = time.perf_counter() - started
    cpu = time.process_time() - cpu
    summary, coverage = snapshot["summary"], snapshot["coverage"]
    print(json.dumps({"strategy": "python-reference", "label": label,
                      "cache_label": "warm-uncontrolled", "wall_seconds": wall,
                      "files_per_second": summary["file_count"] / wall,
                      "cpu_seconds": cpu, "peak_rss_bytes": peak_rss(),
                      "observed": {"files": summary["file_count"], "directories": summary["directory_count"],
                                   "logical_bytes": summary["logical_bytes"], "skipped_reparse": coverage["skipped_count"]},
                      "coverage": {key: coverage[key] for key in ("limit_hit", "examined_entries", "skipped_count", "error_count", "max_entries", "max_depth", "complete_under_policy", "unqualified_complete")}}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plan", type=Path)
    parser.add_argument("--root-log", type=Path, required=True)
    parser.add_argument("--synthetic", action="store_true")
    parser.add_argument("--python-child", help=argparse.SUPPRESS)
    parser.add_argument("--label", help=argparse.SUPPRESS)
    args = parser.parse_args()
    require_non_elevated()
    if args.python_child:
        python_child(args.python_child, args.label, args.root_log)
        return
    if not args.plan or not args.plan.is_absolute() or args.plan.is_relative_to(REPO):
        parser.error("plan must be an absolute path outside this public worktree")
    roots = json.loads(args.plan.read_text(encoding="utf-8"))
    assert len(roots) in (3, 4) and [r["label"] for r in roots] == [f"real-{chr(65+i)}" for i in range(len(roots))]
    receipt = {"schema_version": 1, "platform": platform.platform(), "python": platform.python_version(),
               "measured_at_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
               "token_elevated": False, "logical_processors": os.cpu_count(),
               "source_base": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip(),
               "source_sha256": {str(p.relative_to(REPO)).replace("\\", "/"): hashlib.sha256(p.read_bytes()).hexdigest()
                                 for p in sorted((REPO / "crates/loomward-lab/src").glob("*.rs"))},
               "executable_sha256": hashlib.sha256(EXE.read_bytes()).hexdigest(),
               "cache_method": "First traversal of each chosen root by this session, not known first touch since boot. Only std-single is first-touch; the other three strategies inherit its cache. Five additional warm-uncontrolled sweeps. No flush, reboot, no-buffering claim, or elevation.",
               "roots": [], "synthetic": [], "complete": False}
    save(receipt)
    for root in roots:
        item = {"label": root["label"], "storage": root["storage"], "passes": []}
        receipt["roots"].append(item)
        save(receipt)
        for trial in range(6):
            cache = root.get("initial_cache_label", "first-session-touch-uncontrolled") if trial == 0 else "warm-uncontrolled"
            result = native("cross-check", "--root", root["path"], "--root-log", args.root_log,
                            "--cache-label", cache, "--threads", 8, "--buffer-kib", 64)
            result["pass"] = trial
            item["passes"].append(result)
            save(receipt)
            timing = ", ".join(f"{r['strategy']} {r['wall_seconds']:.3f}s" for r in result["runs"])
            print(f"{item['label']} pass {trial}: agreement={result['agreement']}; {timing}", flush=True)
        item["stable_across_passes"] = all(
            p["agreement"] and p["runs"][0]["observed"] == item["passes"][0]["runs"][0]["observed"]
            for p in item["passes"])
        save(receipt)
    smallest = min(receipt["roots"], key=lambda r: r["passes"][-1]["runs"][0]["observed"]["files"])
    root = next(r for r in roots if r["label"] == smallest["label"])
    run = subprocess.run([sys.executable, __file__, "--python-child", root["path"], "--label", root["label"],
                          "--root-log", str(args.root_log)], cwd=REPO, capture_output=True, text=True, check=True)
    receipt["python_comparison"] = json.loads(run.stdout)
    save(receipt)
    if args.synthetic:
        for drive, storage in (("G", "NVMe"), ("E", "HDD")):
            root = Path(f"{drive}:\\loomward-lab\\scale\\mixed-1000000")
            generated = not root.exists()
            if generated:
                print(f"generating {storage} mixed 1M tree", flush=True)
                manifest = native("generate", "--root", root, "--files", 1000000, "--seed", 42, "--profile", "mixed", "--threads", 8)
            else:
                native("bench", "--root", root, "--strategy", "handle")
                manifest = json.loads((root / "manifest.json").read_text())
            assert manifest["seed"] == 42 and manifest["profile"] == "mixed" and manifest["expected"]["files"] == 1000000
            item = {"storage": storage, "root": str(root), "generated": generated, "manifest": manifest, "runs": []}
            receipt["synthetic"].append(item)
            save(receipt)
            for trial in range(5):
                for strategy in ("std-single", "std-parallel", "find", "handle"):
                    measured = native("bench", "--root", root, "--strategy", strategy, "--threads", 8, "--buffer-kib", 64, "--cache-label", "warm-uncontrolled")
                    measured.pop("root")
                    measured["pass"] = trial + 1
                    item["runs"].append(measured)
                    save(receipt)
                    print(f"{storage} 1M pass {trial+1}: {strategy} {measured['wall_seconds']:.3f}s; exact={measured['exact']}", flush=True)
        assert receipt["synthetic"][0]["manifest"]["expected"] == receipt["synthetic"][1]["manifest"]["expected"]
    receipt["complete"] = True
    save(receipt)


if __name__ == "__main__":
    main()
