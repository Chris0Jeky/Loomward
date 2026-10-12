#!/usr/bin/env python3
"""Generate the winget multi-file manifest (version, installer, defaultLocale) for one Loomward release.

    py -3 packaging/winget/make_manifest.py 0.3.0-preview.1 --out <dir>
    py -3 packaging/winget/make_manifest.py 0.3.0-preview.1 --out <dir> --sums-file SHA256SUMS.txt
    py -3 packaging/winget/make_manifest.py 0.3.0-preview.1 --out <dir> --sha256 <64 hex> --release-date 2026-10-20

The installer hash comes from the release's own SHA256SUMS.txt (downloaded unless --sums-file or
--sha256 is given). Output: <out>/manifests/c/Chris0Jeky/Loomward/<version>/, ready to copy into a
microsoft/winget-pkgs checkout. Standard library only; nothing is published. See README.md here.
"""
import argparse
import json
import re
import sys
import urllib.request
from pathlib import Path

MANIFEST_VERSION = "1.12.0"  # newest schema with published JSON schemas and in use in winget-pkgs (README.md)
PACKAGE_ID = "Chris0Jeky.Loomward"
REPO = "https://github.com/Chris0Jeky/Loomward"
VERSION_RE = re.compile(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?")
SHA_RE = re.compile(r"[0-9A-Fa-f]{64}")
DATE_RE = re.compile(r"\d{4}-\d{2}-\d{2}")


def scalar(s):
    """Plain YAML scalar when unambiguous, else a JSON string (valid YAML double-quoted)."""
    if re.fullmatch(r"[A-Za-z0-9._/:@%+-]+", s) and not re.fullmatch(r"[0-9]+([eE][0-9]+)?", s):
        return s
    return json.dumps(s, ensure_ascii=False)


def dump(doc, indent=0):
    """Minimal YAML writer for the dict / list / str shapes used below."""
    pad, out = "  " * indent, []
    for key, val in doc.items():
        if isinstance(val, dict):
            out.append(f"{pad}{key}:")
            out += dump(val, indent + 1)
        elif isinstance(val, list):
            out.append(f"{pad}{key}:")
            for item in val:
                if isinstance(item, dict):
                    lines = dump(item, indent + 1)
                    out.append(f"{pad}- {lines[0].lstrip()}")
                    out += lines[1:]
                else:
                    out.append(f"{pad}- {scalar(item)}")
        else:
            out.append(f"{pad}{key}: {scalar(val)}")
    return out


def sha_from_sums(text, filename):
    # sha256sum lines: "<hash>  <name>" (text mode) or "<hash> *<name>" (binary mode).
    for line in text.splitlines():
        m = re.fullmatch(r"([0-9A-Fa-f]{64}) [ *](.+?)\s*", line)
        if m and m.group(2) == filename:
            return m.group(1).upper()
    raise SystemExit(f"no SHA-256 line for {filename} in SHA256SUMS.txt")


def build(version, sha256, release_date=None):
    tag, setup = f"v{version}", f"Loomward_{version}_x64-setup.exe"
    head = {"PackageIdentifier": PACKAGE_ID, "PackageVersion": version}
    ver = {**head, "DefaultLocale": "en-US", "ManifestType": "version", "ManifestVersion": MANIFEST_VERSION}
    inst = {
        **head,
        "InstallerLocale": "en-US",
        "Platform": ["Windows.Desktop"],
        "InstallerType": "nullsoft",
        "Scope": "user",
        "InstallModes": ["interactive", "silent", "silentWithProgress"],
        "InstallerSwitches": {"Silent": "/S", "SilentWithProgress": "/S"},
        "UpgradeBehavior": "install",
    }
    if release_date:
        inst["ReleaseDate"] = release_date
    inst["Installers"] = [
        {
            "Architecture": "x64",
            "InstallerUrl": f"{REPO}/releases/download/{tag}/{setup}",
            "InstallerSha256": sha256,
        }
    ]
    inst.update(ManifestType="installer", ManifestVersion=MANIFEST_VERSION)
    loc = {
        **head,
        "PackageLocale": "en-US",
        "Publisher": "Loomward contributors",  # equals bundle.publisher in native/tauri.conf.json (ARP publisher)
        "PublisherUrl": REPO,
        "PublisherSupportUrl": f"{REPO}/issues",
        "PackageName": "Loomward",
        "PackageUrl": REPO,
        "License": "AGPL-3.0-only",
        "LicenseUrl": f"{REPO}/blob/{tag}/LICENSE",
        "ShortDescription": "Local-first Windows workspace, storage and resource companion (read-only; unsigned preview).",
        "Description": (
            "Loomward shows where your disk space goes, learns how you organise files, simulates how data could "
            "be balanced across disks and explains what uses memory and processes. It reads metadata only and "
            "never moves, deletes or kills anything. Local-first: no cloud requests, no telemetry."
        ),
        "Moniker": "loomward",
        "Tags": ["storage", "disk-usage", "file-management", "local-first", "windows", "tauri"],
        "ReleaseNotesUrl": f"{REPO}/releases/tag/{tag}",
        "ManifestType": "defaultLocale",
        "ManifestVersion": MANIFEST_VERSION,
    }
    return {
        "": ver,
        ".installer": inst,
        ".locale.en-US": loc,
    }


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("version", help="release version without the leading v, e.g. 0.3.0-preview.1")
    ap.add_argument("--out", required=True, type=Path, help="output root (a winget-pkgs-style manifests/ tree is created)")
    src = ap.add_mutually_exclusive_group()
    src.add_argument("--sums-file", type=Path, help="local copy of the release's SHA256SUMS.txt")
    src.add_argument("--sha256", help="installer SHA-256 (64 hex), instead of reading SHA256SUMS.txt")
    ap.add_argument("--release-date", help="YYYY-MM-DD, e.g. from `gh release view v<version> --json publishedAt`")
    a = ap.parse_args(argv)

    if not VERSION_RE.fullmatch(a.version):
        sys.exit(f"version must look like 0.3.0 or 0.3.0-preview.1 (got {a.version!r})")
    if a.release_date and not DATE_RE.fullmatch(a.release_date):
        sys.exit("--release-date must be YYYY-MM-DD")
    if a.sha256:
        if not SHA_RE.fullmatch(a.sha256):
            sys.exit("--sha256 must be 64 hex characters")
        sha = a.sha256.upper()
    else:
        if a.sums_file:
            text = a.sums_file.read_text(encoding="utf-8")
        else:
            url = f"{REPO}/releases/download/v{a.version}/SHA256SUMS.txt"
            with urllib.request.urlopen(url, timeout=30) as r:  # https only; a 404 means no such release
                text = r.read().decode("utf-8")
        sha = sha_from_sums(text, f"Loomward_{a.version}_x64-setup.exe")

    folder = a.out / "manifests" / "c" / "Chris0Jeky" / "Loomward" / a.version
    folder.mkdir(parents=True, exist_ok=True)
    for suffix, doc in build(a.version, sha, a.release_date).items():
        schema = {"": "version", ".installer": "installer", ".locale.en-US": "defaultLocale"}[suffix]
        head = f"# yaml-language-server: $schema=https://aka.ms/winget-manifest.{schema}.{MANIFEST_VERSION}.schema.json\n\n"
        path = folder / f"{PACKAGE_ID}{suffix}.yaml"
        path.write_text(head + "\n".join(dump(doc)) + "\n", encoding="utf-8", newline="\n")
        print(path)


if __name__ == "__main__":
    main()
