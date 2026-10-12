# winget

`make_manifest.py` writes the three-file multi-manifest for `Chris0Jeky.Loomward` from a published release.
Standard library only; nothing is uploaded.

```powershell
# hash read from the release's own SHA256SUMS.txt (downloaded over HTTPS)
py -3 packaging/winget/make_manifest.py 0.3.0-preview.1 --out $env:TEMP\lw-winget `
    --release-date (gh release view v0.3.0-preview.1 --repo Chris0Jeky/Loomward --json publishedAt --jq '.publishedAt[0:10]')
# or offline: --sums-file SHA256SUMS.txt   |   --sha256 <64 hex>
winget validate --manifest $env:TEMP\lw-winget\manifests\c\Chris0Jeky\Loomward\0.3.0-preview.1
py -3 packaging/winget/test_make_manifest.py        # the generator's own checks
```

Output is `manifests/c/Chris0Jeky/Loomward/<version>/` with `Chris0Jeky.Loomward.yaml` (version),
`.installer.yaml` and `.locale.en-US.yaml` (defaultLocale), the layout winget-pkgs expects.

## What the manifest says, and why

| Field | Value | Basis |
|---|---|---|
| `ManifestVersion` | `1.12.0` | winget-pkgs' own `Authoring.md` still links the manifest specification at `manifest/schema/1.12.0`, Microsoft Learn's manifest page (page dated 2026-09-14) shows 1.12.0, and the newest merged PowerToys manifest (0.101.2362.0, 2026-08) uses it. A `1.28.0` schema folder also exists in winget-pkgs (client 1.28), but nothing needs it; the fields used are all in 1.12.0 |
| `InstallerType` | `nullsoft` | Tauri's NSIS output. For Nullsoft the client supplies `/S` itself; the explicit `Silent`/`SilentWithProgress: /S` follows the brief and the community-repo rule that both modes must work |
| `Scope` | `user` | `bundle.windows.nsis.installMode: currentUser`, no administrator rights (`RELEASING.md`) |
| `InstallerUrl` | `https://github.com/Chris0Jeky/Loomward/releases/download/v<version>/Loomward_<version>_x64-setup.exe` | the release asset; the validator wants the publisher's own direct HTTPS URL without redirectors |
| `InstallerSha256` | from `SHA256SUMS.txt`, upper-cased | `release.yml` writes it with `sha256sum` after the build |
| `License` / `LicenseUrl` | `AGPL-3.0-only` / `.../blob/v<version>/LICENSE` | `LICENSE`, `native/Cargo.toml` |
| `Publisher` | `Loomward contributors` | equals `bundle.publisher`, which is what the installer writes to Add/Remove Programs |
| `ReleaseNotesUrl` | `.../releases/tag/v<version>` | the pre-release page |

Deliberately left out until a real install shows the values: `ProductCode` and `AppsAndFeaturesEntries` (winget
uses them to match an installed app to the manifest for `upgrade`; the NSIS uninstall key name and `DisplayVersion`
are unmeasured), `MinimumOSVersion`, and `ElevationRequirement`. After the first real install, read
`reg query HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall /s /f Loomward` and add what you see.

## Preview versions in winget

- **Ordering.** winget splits the version at `.`, parses a leading integer from each part, and ranks a part *with*
  a trailing string below the same part without one ([Authoring Manifests, "Version Sorting in WinGet"](https://github.com/microsoft/winget-pkgs/blob/master/doc/Authoring.md);
  retrieved 2026-10-12). By that algorithm (derived, not run) `0.3.0-preview.1` < `0.3.0-preview.2` <
  `0.3.0-preview.10` < `0.3.0`, so a later stable release supersedes the previews under the same identifier.
- **No pre-release flag.** The schema's `Channel` key is documented as not implemented
  ([installer schema 1.28.0](https://github.com/microsoft/winget-pkgs/blob/master/doc/manifest/schema/1.28.0/installer.md)),
  so `winget install Chris0Jeky.Loomward` installs the highest version published. While only previews exist, that
  is an unsigned preview. Say so in the PR description and the locale `Description` if that matters.
- **Policy.** I found no rule against a pre-release `PackageVersion` in `Policies.md`, `Authoring.md` or
  `Validation.md`; acceptance is the moderators' call (`Moderation.md`). Hypothesis: accepted. Settled only by a PR.
- **Unsigned.** The validation steps I read (manifest, URLs, domain, installers scan, install, uninstall) list no
  signature check; a search of `Validation.md` and `ValidationFailureGuide.md` for "sign" finds only MSIX signatures. The risks are an antivirus/Defender heuristic on a fresh unsigned binary
  (`Binary-Validation-Error`, `Validation-Defender-Error`), the `Validation-Uninstall-Error` label ("did not clean up
  completely": Loomward keeps `%LOCALAPPDATA%\Loomward Data` on purpose), and the first-run SmartScreen prompt, which
  winget does not remove. Put the state-retention in the PR text.
- Sources: [Policies.md](https://github.com/microsoft/winget-pkgs/blob/master/doc/Policies.md),
  [Validation.md](https://github.com/microsoft/winget-pkgs/blob/master/doc/Validation.md),
  [ValidationFailureGuide.md](https://github.com/microsoft/winget-pkgs/blob/master/doc/ValidationFailureGuide.md),
  [Learn: submit to the repository](https://learn.microsoft.com/en-us/windows/package-manager/package/repository) (page
  dated 2024-11-21, updated 2026-07-14), all retrieved 2026-10-12.

## Submission steps

Needs: a published release, the owner's go-ahead to open a PR in a third-party repository, and the owner's one-time
Microsoft CLA acceptance (a `Needs-CLA` label blocks the merge until the GitHub account signs at
<https://cla.opensource.microsoft.com/microsoft/winget-pkgs>; that step is human-only).

1. Cut the release; install it once by hand and note the Add/Remove Programs entry and the state folder.
2. Generate and `winget validate` as above. `AGENTS.md` in winget-pkgs asks for exactly this, one package per PR, and
   a summary of the validation done.
3. Optional: `Tools\SandboxTest.ps1 <manifest folder>` from a winget-pkgs checkout (needs Windows Sandbox).
4. Fork and open the PR (agent-doable with `gh`, one version per PR, nothing else in the diff):

   ```sh
   gh repo fork microsoft/winget-pkgs --clone=false
   git clone --filter=blob:none --no-checkout https://github.com/Chris0Jeky/winget-pkgs.git wp && cd wp
   git sparse-checkout set manifests/c/Chris0Jeky/Loomward && git checkout master
   cp -r "$OUT/manifests/c/Chris0Jeky/Loomward/<version>" manifests/c/Chris0Jeky/Loomward/
   git switch -c Chris0Jeky.Loomward-<version> && git add . && git commit -m "New package: Chris0Jeky.Loomward version <version>"
   git push -u origin HEAD
   gh pr create --repo microsoft/winget-pkgs --title "New package: Chris0Jeky.Loomward version <version>" --body "..."
   ```

   (`wingetcreate submit --prtitle ... --token ... <folder>` does the same with a classic PAT (permissions in the winget-create README);
   [submit.md](https://github.com/microsoft/winget-create/blob/main/doc/submit.md).) Later versions of an existing
   package: rerun the generator with the new version and use `New version:` in the title, or
   `wingetcreate update Chris0Jeky.Loomward --version <v> --urls <installer url> --submit`
   ([update.md](https://github.com/microsoft/winget-create/blob/main/doc/update.md)).
5. Watch the PR labels (`Azure-Pipeline-Passed`, `Needs-Author-Feedback`, ...); a moderator re-runs validation with
   `@wingetbot run`. Unanswered `Needs-Author-Feedback` closes the PR after 10 days.

## Proof of this change (2026-10-12, winget 1.29.380)

Run against the real pre-release
[v0.3.0-preview.1](https://github.com/Chris0Jeky/Loomward/releases/tag/v0.3.0-preview.1):

- `make_manifest.py 0.3.0-preview.1 --out <temp> --release-date 2026-10-12` downloaded the release's
  `SHA256SUMS.txt` (its line uses the `hash *name` binary form) and wrote `InstallerSha256`
  `5B5AF7DC431FA166FA0C3E4D31873F3DBE57E50D589A38835CFB0C5E6F77E490`; `winget hash` on the downloaded installer and
  `sha256sum` give the same value.
- The three files parse with PyYAML and validate against `manifest.{version,installer,defaultLocale}.1.12.0.json`
  from microsoft/winget-cli (0 errors each); `winget validate --manifest` printed "Manifest validation succeeded.".
- Negative control: the same folder with a non-hex `InstallerSha256` fails `winget validate`.
- `InstallerUrl` answers 200 after exactly one redirect (github.com to `release-assets.githubusercontent.com`), the
  usual GitHub-release shape (the PowerToys manifest uses it); whether winget-pkgs' `Validation-Indirect-URL` check
  accepts it for this package is only settled by a PR.
- 3 unit tests (`test_make_manifest.py`) pass.

Nothing was submitted: the first winget publication is an owner re-review trigger (invariant 8). Not verified: a PR
through winget-pkgs' pipeline, `winget install` of the release, the Add/Remove Programs key. The installer is
unsigned (`NotSigned`).
