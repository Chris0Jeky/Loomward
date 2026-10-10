"""Synthetic native controls for manifest-free scans; no real metadata is exported."""
import argparse
import csv
import io
import json
import os
from pathlib import Path
import subprocess

from check_windows import call
from measure import REPO, require_non_elevated


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root-log", type=Path, required=True)
    args = parser.parse_args()
    require_non_elevated()
    root = Path(r"G:\loomward-lab\scale\cross-check-control")
    # create_new + the existing marker/oracle prevent taking over an existing tree.
    call("generate", "--root", root, "--files", 3, "--seed", 42, "--profile", "mixed")
    data = root / "data"
    cases = []

    def check():
        run = call("cross-check", "--root", data, "--root-log", args.root_log)
        result = json.loads(run.stdout)
        assert result["agreement"] and not result["disagreements"]
        assert len(result["runs"]) == 4
        totals = result["runs"][0]["observed"]
        assert totals["files"] == 3
        return totals

    try:
        base = check()
        cases.append({"case": "manifest-free-synthetic-all-strategies", "passed": True})
        junction = data / "junction-control"
        subprocess.run(["cmd.exe", "/c", "mklink", "/J", str(junction), str(root)], cwd=REPO,
                       check=True, capture_output=True)
        try:
            totals = check()
            assert totals == {**base, "skipped_reparse": 1}
            cases.append({"case": "cyclic-junction-skipped-not-followed", "passed": True})
            call("cross-check", "--root", junction, "--root-log", args.root_log, ok=False)
            cases.append({"case": "junction-root-refused", "passed": True})
        finally:
            os.rmdir(junction)
        denied = data / "denied-control"
        denied.mkdir()
        account = subprocess.check_output(["whoami.exe", "/user", "/fo", "csv", "/nh"], text=True)
        sid = next(csv.reader(io.StringIO(account)))[1]
        subprocess.run(["icacls.exe", str(denied), "/deny", f"*{sid}:(RD)"], check=True, capture_output=True)
        try:
            totals = check()
            assert totals == {**base, "directories": base["directories"] + 1, "denied_directories": 1}
            cases.append({"case": "denied-directory-counted-consistently", "passed": True})
        finally:
            subprocess.run(["icacls.exe", str(denied), "/remove:d", f"*{sid}"], check=True, capture_output=True)
            denied.rmdir()
        # The log is synthetic here; assert every strategy recorded its scope.
        lines = args.root_log.read_text(encoding="utf-8").splitlines()
        for strategy in ("std-single", "std-parallel", "find", "handle"):
            assert any(str(data) in line and f"; {strategy};" in line for line in lines)
        cases.append({"case": "each-strategy-records-root", "passed": True})
        for path in ("C:\\", r"C:\Users\fixture", r"E:\Browser", r"E:\keys"):
            call("cross-check", "--root", path, "--root-log", args.root_log, ok=False)
        cases.append({"case": "broad-and-protected-scopes-refused", "passed": True})
        call("cross-check", "--root", data, "--root-log", data / "unsafe.log", ok=False)
        assert not (data / "unsafe.log").exists()
        cases.append({"case": "state-inside-root-refused-before-write", "passed": True})
    finally:
        call("destroy", "--root", root)
    assert not root.exists()
    cases.append({"case": "marked-synthetic-control-cleaned", "passed": True})
    output = REPO / "evidence/v3/real-folder-stress-checks.json"
    output.write_text(json.dumps({"platform": "Windows 11, NTFS, non-elevated", "cases": cases}, indent=2) + "\n")
    print(f"PASS {len(cases)} native cross-check controls")


if __name__ == "__main__":
    main()
