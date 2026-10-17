"""Build and stage the host's Ask broker for Tauri externalBin (no publishing)."""
import argparse
import json
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def build(release=False):
    # Use the desktop target directory, including CARGO_TARGET_DIR, so CI shares its cache.
    metadata = subprocess.check_output([
        "cargo", "metadata", "--locked", "--no-deps", "--format-version", "1",
        "--manifest-path", str(ROOT / "native/Cargo.toml"),
    ], cwd=ROOT, text=True)
    target = Path(json.loads(metadata)["target_directory"])
    # shortcut: host builds only; add an explicit target when the desktop pipeline cross-compiles.
    triple = subprocess.check_output(["rustc", "--print", "host-tuple"], cwd=ROOT, text=True).strip()
    command = [
        "cargo", "build", "--locked", "--manifest-path", str(ROOT / "Cargo.toml"),
        "-p", "loomward-broker", "--target-dir", str(target),
    ]
    if release:
        command.append("--release")
    subprocess.run(command, cwd=ROOT, check=True)
    suffix = ".exe" if "windows" in triple else ""
    source = target / ("release" if release else "debug") / f"loomward-broker{suffix}"
    staged = ROOT / "native/binaries" / f"loomward-broker-{triple}{suffix}"
    staged.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, staged)
    print(f"Staged {staged}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release", action="store_true")
    build(parser.parse_args().release)
