"""Record aggregate-only L17 scan evidence from an already generated L tier."""
import argparse
import hashlib
import json
import platform
import subprocess
from datetime import datetime, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def read_json(path):
    raw = path.read_bytes()
    return json.loads(raw.decode("utf-16" if raw.startswith((b"\xff\xfe", b"\xfe\xff")) else "utf-8-sig"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--first-after-generate", action="store_true")
    args = parser.parse_args()
    generation = read_json(ROOT / ".loomward/generate-10m.json")
    process = read_json(ROOT / ".loomward/generate-10m-process.json")
    assert generation["complete"] and generation["expected"]["files"] == 10_000_000
    assert generation["profile"] == "mixed" and generation["seed"] == 42
    assert process["exit_code"] == 0
    binary = args.binary.resolve()
    command = [str(binary), "bench", "scan", "--tier", "L", "--runs", "20"]
    if args.first_after_generate:
        command += ["--cache-label", "first-after-generate"]
    # stderr streams progress; stdout contains one aggregate-only JSON result.
    result = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, check=True, text=True)
    data = json.loads(result.stdout)
    runs = data["runs"]
    assert len(runs) >= 20 and len({r["process_id"] for r in runs}) == len(runs)
    assert data["root"] == "10M lab tier (E: scale lab)"
    assert data["oracle_passed_all_runs"]
    assert all(r["oracle"] == "exact-manifest" if r["complete"] else
               r["limit_hit"] and not r["sweep_allowed"] and r["allocated_bytes"] is None
               for r in runs)
    data["recorded_utc"] = datetime.now(timezone.utc).isoformat()
    data["machine"] = {
        "os": platform.platform(),
        "hardware": "withheld from public evidence (evidence/v3/README.md, 10 October 2026 correction)",
        "state_disk": "none; aggregate-only memory sink",
    }
    data["generation"] = {
        "profile": "mixed", "seed": 42, "threads": 8,
        "host": "shared-host/uncontrolled", "oracle": "exact deterministic manifest",
        "expected": generation["expected"],
        "wall_seconds_including_oracle": generation["generation_seconds"],
        "files_per_second_including_oracle": 10_000_000 / generation["generation_seconds"],
        "process_wall_seconds": process["process_wall_seconds"],
        "process_files_per_second": 10_000_000 / process["process_wall_seconds"],
        "payload_allocation_bytes": generation["file_data_allocation_bytes"],
        "observed_volume_delta_bytes": generation["volume_free_before"] - generation["volume_free_after"],
        "free_space_reserve_bytes": 20_000_000_000,
        "payload_volume_delta_cap_bytes": 66_666_666_667,
    }
    data["build"] = {
        "base_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "uncommitted": True,
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "generation_binary_sha256": read_json(ROOT / ".loomward/generation-build.json")["generation_binary_sha256"],
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "source_sha256": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in [
            "crates/loomward-lab/src/lib.rs", "crates/loomward-lab/src/windows.rs",
            "crates/loomward-engine/src/scan/pipeline.rs", "crates/loomward-engine/src/scan/watch.rs",
        ]},
    }
    data["limitations"] = [
        "shared CPU/disk activity; warm/uncontrolled cache; no cold boot or cache flush",
        "partial oracle validates invariants and bounds, not an exact scheduled subset",
        "enumeration wall overlaps staging; worker seconds include backpressure",
        "no SQLite persistence or state-disk I/O; no catalogue benchmark",
        "lifetime peak memory includes manifest oracle validation and scan setup",
        "scan wall is run_scan only; excludes CLI manifest planning, process startup and grant construction",
        "mixed lab uses zero sparse payloads, 256 files/bucket and occasional 40-component paths; not every doc 41 distribution",
        "an E: scale-lab run does not qualify the G: warm full-scan P2 target",
        "no WinDirStat comparison, cold-cache, reboot, elevation or standby-list flush",
    ]
    data["initial_attempt"] = {
        "cache": "first-after-generate", "included_in_summary": False,
        "status": "validator rejected failed/skipped/sink group before recording timings",
        "diagnosis": "small reproducer proved expected limit listings incremented failed; corrected counting preserves real errors",
        "timings_retained": False,
    }
    output = ROOT / "evidence/v3/bench/scan-10m.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"runs": len(runs), "p50_wall_seconds": data["median_wall_seconds"],
                      "p95_wall_seconds": data["p95_wall_seconds"],
                      "exact_manifest": data["manifest_matched_all_runs"]}))


if __name__ == "__main__":
    main()
