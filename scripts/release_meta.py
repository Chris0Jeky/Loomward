"""Release metadata for the preview and nightly channels (HUMAN_TODO q-30, q-31, q-33).

Used by .github/workflows/release.yml and nightly.yml; stdlib only, so it runs on the runner's
Python and locally as a dry run:

  py -3 scripts/release_meta.py nightly-version --date 20261012 --run 7
  py -3 scripts/release_meta.py notes --version V --ref SHA --sha SHA [--nightly] > notes.md
  py -3 scripts/release_meta.py latest-json --version V --tag nightly --installer X.exe --notes TEXT > latest.json
  py -3 scripts/release_meta.py retag latest.json --tag nightly-previous

`latest.json` is the Tauri updater's static manifest; native/src/updates.rs reads it from
`releases/download/<preview-latest|nightly|nightly-previous>/latest.json`.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import sys
from pathlib import Path
from urllib.parse import quote

ROOT = Path(__file__).resolve().parents[1]
REPO = "Chris0Jeky/Loomward"
CORE = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
# The updater's target for an x64 NSIS install (tauri-plugin-updater tries `-nsis` first).
TARGETS = ("windows-x86_64-nsis", "windows-x86_64")


def nightly_version(core: str, date: str, run: int) -> str:
    """`<core>-nightly.<YYYYMMDD>.<run>`: semver numeric identifiers, so a later day or run sorts
    higher. The core is the app's product version (app/package.json), which matches the previews;
    the Cargo workspace still says 0.1.0."""
    if not CORE.match(core):
        raise ValueError(f"not a plain major.minor.patch version: {core!r}")
    dt.datetime.strptime(date, "%Y%m%d")
    if not re.fullmatch(r"\d{8}", date) or run < 1:
        raise ValueError("date is YYYYMMDD and run is a positive integer")
    return f"{core}-nightly.{date}.{run}"


def app_version() -> str:
    return json.loads((ROOT / "app" / "package.json").read_text(encoding="utf-8"))["version"]


def render_notes(template: str, version: str, ref: str, sha: str, banner: str | None) -> str:
    body = template.replace("{{VERSION}}", version).replace("{{TAG}}", ref).replace("{{SHA}}", sha)
    return (banner.replace("{{VERSION}}", version).replace("{{SHA}}", sha) + "\n" + body) if banner else body


def latest_json(version: str, tag: str, installer: str, signature: str, notes: str, pub_date: str,
                repo: str = REPO) -> dict:
    url = f"https://github.com/{repo}/releases/download/{tag}/{quote(Path(installer).name)}"
    entry = {"signature": signature.strip(), "url": url}
    return {"version": version, "notes": notes, "pub_date": pub_date,
            "platforms": {t: dict(entry) for t in TARGETS}}


def retag(manifest: dict, tag: str) -> dict:
    """Points every platform URL at another release tag (the rotated previous nightly)."""
    out = json.loads(json.dumps(manifest))
    for entry in out["platforms"].values():
        entry["url"] = re.sub(r"/releases/download/[^/]+/", f"/releases/download/{tag}/", entry["url"], count=1)
    return out


def main(argv: list[str]) -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    n = sub.add_parser("nightly-version")
    n.add_argument("--date", required=True)
    n.add_argument("--run", type=int, required=True)
    r = sub.add_parser("notes")
    r.add_argument("--version", required=True)
    r.add_argument("--ref", required=True, help="tag or commit the links point at")
    r.add_argument("--sha", required=True)
    r.add_argument("--nightly", action="store_true")
    j = sub.add_parser("latest-json")
    j.add_argument("--version", required=True)
    j.add_argument("--tag", required=True)
    j.add_argument("--installer", required=True)
    j.add_argument("--notes", required=True)
    j.add_argument("--repo", default=REPO)
    t = sub.add_parser("retag")
    t.add_argument("manifest")
    t.add_argument("--tag", required=True)
    a = p.parse_args(argv)
    if a.cmd == "nightly-version":
        print(nightly_version(app_version(), a.date, a.run))
    elif a.cmd == "notes":
        rel = ROOT / ".github" / "release"
        banner = (rel / "nightly-banner.md").read_text(encoding="utf-8") if a.nightly else None
        sys.stdout.write(render_notes((rel / "preview-notes.md").read_text(encoding="utf-8"),
                                      a.version, a.ref, a.sha, banner))
    elif a.cmd == "latest-json":
        sig = Path(a.installer + ".sig").read_text(encoding="utf-8")
        now = dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        print(json.dumps(latest_json(a.version, a.tag, a.installer, sig, a.notes, now, a.repo), indent=2))
    else:
        manifest = json.loads(Path(a.manifest).read_text(encoding="utf-8"))
        print(json.dumps(retag(manifest, a.tag), indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
