# Scoop bucket

The bucket is this repository: the manifest is [`bucket/loomward.json`](../../bucket/loomward.json) (Scoop finds a
`bucket/` folder at the root of any bucket repository).

```powershell
scoop bucket add loomward https://github.com/Chris0Jeky/Loomward
scoop install loomward/loomward
```

`scoop bucket add` clones the whole repository, history included; a dedicated `scoop-loomward` repository would
make that small if it ever matters.

## Why extract with 7-Zip instead of running the installer

The manifest downloads the NSIS installer with the `#/dl.7z` suffix, so Scoop unpacks it with 7-Zip instead of
running it. Scoop's own manifest guide recommends exactly this "to bypass executable installers which might have
undesirable side-effects like registry changes, files placed outside the install directory, or an admin elevation
prompt" ([App Manifests, `url`](https://github.com/ScoopInstaller/Scoop/wiki/App-Manifests); wiki checkout of
2025-05-21). Against the `installer` + `/S` route it gives:

- no registry or Start-menu writes by the installer, and no asynchronous uninstaller. `RELEASING.md` documents that
  the NSIS uninstaller re-runs itself from TEMP and returns at once, which races with Scoop deleting the folder;
- `scoop update`/`uninstall`/version folders and the `current` junction work as for every portable app;
- the `uninstall.exe` an NSIS install writes at install time is not needed at all.

Costs, accepted: the embedded WebView2 bootstrapper is skipped (the manifest `suggest`s `extras/webview2` and says
so in `notes`), and the app has no Add/Remove Programs entry (Scoop's list is the entry). Installing the same
release through winget as well would be two copies; pick one channel per machine.

**`persist` is not needed.** State is `%LOCALAPPDATA%\Loomward Data\v3\<class>`, derived from `LOCALAPPDATA`, never
from the executable's folder (`native/src/launch.rs:76-97`; `RELEASING.md`). `scoop uninstall loomward` therefore
keeps the user's state, like the NSIS uninstaller does. The contract examples are the one thing that must sit
beside the exe (`launch.rs:207-217`), and `pre_install` refuses to finish unless the extraction kept them.

`pre_install` removes the `$PLUGINSDIR` and `$TEMP` folders 7-Zip unpacks (`$TEMP` holds the embedded WebView2 bootstrapper), then **fails closed** unless
`loomward-desktop.exe` and `contracts\v3\examples\commands` are present, so a layout change breaks the install
loudly instead of shipping a broken app.

## Version detection and updates

`checkver` reads `https://api.github.com/repos/Chris0Jeky/Loomward/releases` with `jsonpath: $[0].tag_name` and a
regex that accepts `v0.3.0-preview.1` as well as `v0.3.0`. The shorthand `"checkver": "github"` is not used
because it ignores pre-releases ([App Manifest Autoupdate](https://github.com/ScoopInstaller/Scoop/wiki/App-Manifest-Autoupdate),
"Pre-releases will be ignored!"). The list is newest-first and the API returns no drafts without a token. With `SCOOP_GH_TOKEN` set, the owner's draft
releases are visible too, so check `git diff bucket/loomward.json` names a published tag before committing.
`autoupdate` rebuilds the URL from `$version` and reads the hash from the release's `SHA256SUMS.txt` (`$baseurl`
mode `extract`, which finds the `sha256sum` line by file name).

## First release and every release after

The committed manifest is a placeholder (`0.0.0-unreleased`, all-zero hash) that fails closed. With the owner's go-ahead (a release now
exists), with Scoop installed:

```powershell
$scoop = "$env:USERPROFILE\scoop\apps\scoop\current\bin"
& "$scoop\checkver.ps1" -Dir bucket -App loomward -Update -ForceUpdate   # fills version, url, hash
git diff bucket/loomward.json                                            # review, then commit
scoop install .\bucket\loomward.json                                     # smoke: install, start, uninstall
```

Set `SCOOP_GH_TOKEN` (or `scoop config gh_token`) if the unauthenticated API limit bites. Automating this on a
schedule is Scoop's `excavator` workflow (BucketTemplate), which needs a `.github/workflows` file and was left
out here on purpose.

## Proof of this change (2026-10-12)

Run against the real pre-release
[v0.3.0-preview.1](https://github.com/Chris0Jeky/Loomward/releases/tag/v0.3.0-preview.1) (installer 6,525,857 bytes,
SHA-256 `5b5af7dc...e490`, published 2026-10-12T00:26:36Z).

| Claim | Check | Result |
|---|---|---|
| Manifest is valid Scoop JSON | `jsonschema` against `schema.json` from ScoopInstaller/Scoop `master` (blob `9457c391`) | 0 errors |
| `checkver` finds the pre-release | local Scoop (checkout of 2024-07-30) `checkver.ps1 -Update -ForceUpdate` on a scratch copy, live GitHub API | `0.3.0-preview.1 (scoop version is 0.0.0-unreleased) autoupdate available` |
| `autoupdate` URL and hash | same run | url `.../v0.3.0-preview.1/Loomward_0.3.0-preview.1_x64-setup.exe#/dl.7z`; hash `5b5af7dc...e490` found in `SHA256SUMS.txt` ("Extract Mode"); it equals `sha256sum` of the downloaded installer |
| 7-Zip layout assumption | `7z x` of the real installer | `loomward-desktop.exe`, `contracts/v3/examples/commands/*`, plus `$PLUGINSDIR` and `$TEMP/MicrosoftEdgeWebview2Setup.exe` |
| `pre_install` | the manifest's own `pre_install` lines run in PowerShell on that extraction | removes both `$` folders, passes the layout check, leaves `contracts` and `loomward-desktop.exe` |

**The committed manifest stays the placeholder on purpose.** Filling it in (the `checkver -u` run above) is the first
publication of the Scoop channel, which the owner re-reviews (invariant 8). Run the "First release" command above
after that go-ahead; its output is the manifest the checks above produced.

Not verified: `scoop install loomward` itself (it would install 7-Zip into this machine's Scoop and run the app's
shortcut step; left to the owner's go-ahead or a throwaway machine), Scoop's current (2026) behaviour (local Scoop is
2024), and the app starting from a Scoop folder. The installer is unsigned (`Get-AuthenticodeSignature` status
`NotSigned`).
