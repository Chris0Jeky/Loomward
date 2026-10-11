# Releasing a Loomward desktop preview

Owner decision (HUMAN_TODO q-17, 2026-10-11): Loomward publishes an **unsigned preview
pre-release** of the Tauri desktop shell on GitHub. The workflow is
[`.github/workflows/release.yml`](.github/workflows/release.yml); it never runs on a push to a branch
or a pull request.

## Cut a preview

1. Land what you want in the preview on `main`. The workflow refuses any commit that is not in
   `main`'s history.
2. Pick the next free version, `<major>.<minor>.<patch>-preview.<n>`, for example `0.3.0-preview.1`.
3. Dispatch it, with `main` as the ref:

   ```sh
   gh workflow run release.yml --repo Chris0Jeky/Loomward --ref main -f version=0.3.0-preview.1
   ```

   Pushing the tag `v0.3.0-preview.1` on a `main` commit does the same. A dispatch creates the tag at
   the dispatched commit.

The job runs on `windows-latest` (60-minute limit; the first run has not been timed). It checks the version, builds `app/dist`
and the NSIS installer, self-tests the release binary, silently installs the installer, self-tests the
installed app with the checkout's contract examples removed (it reads its bundled copy and opens the
engine service in a scratch state directory), starts the installed app with its default personal
engine session, then creates the pre-release. The runner is elevated, so this start grants nothing:
native identity opens are refused there. A dispatch that fails before the last step leaves no
release and no tag, so fix and re-dispatch. A run started by pushing a tag leaves that tag behind, and
an interrupted `gh release create` can leave a partial release; clean both up as in *Roll back* before
re-running the same version.

## What the pre-release holds

| Asset | What it is |
|---|---|
| `Loomward_<version>_x64-setup.exe` | NSIS installer: per-user (`%LOCALAPPDATA%`), no administrator rights, **unsigned** (SmartScreen warns) |
| `SHA256SUMS.txt` | SHA-256 of the installer, one `sha256sum` line |

The release body (`.github/release/preview-notes.md`) says the build is unsigned; that it runs the
engine on the tester's machine and reads only folders granted through the Windows folder picker,
metadata only; that the interface cannot open that picker yet; that it has no file or process
effect; and where its state lives. It links the evidence index and the issue tracker. The app's
state is under `%LOCALAPPDATA%\Loomward\v3\` (the per-user install directory is
`%LOCALAPPDATA%\Loomward`); uninstalling removes the program files, not that state.
There is no MSI: WiX rejects a text pre-release such as `-preview.1` in an MSI version.

## Verify a download

```powershell
Get-FileHash .\Loomward_0.3.0-preview.1_x64-setup.exe -Algorithm SHA256
Get-Content .\SHA256SUMS.txt
```

The two hashes must be equal. (With Git Bash: `sha256sum -c SHA256SUMS.txt`.) The checksum file sits
next to the installer on the same release, so it proves an intact download, not who built it; the
provenance is the commit named in the release body.

## Roll back

Delete the release (if one exists) and its tag, then re-dispatch, with a fresh number if the old
one was ever public:

```sh
gh release delete v0.3.0-preview.1 --repo Chris0Jeky/Loomward --cleanup-tag --yes
git push --delete origin v0.3.0-preview.1
```

The second command is needed when no release exists (a failed tag-push run); it fails harmlessly
when `--cleanup-tag` already removed the tag.

The workflow refuses to reuse a version whose tag or release still exists. Do not move or re-push an
existing tag.

## Signing is a future owner action

Nothing here signs. Signing needs a certificate the owner chooses and pays for (an OV or EV
certificate, or Azure Trusted Signing), a repository secret, and a release-workflow change that is
reviewed as a tier decision. Until then every release stays labelled "unsigned preview".
