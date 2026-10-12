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
engine session and checks that its store exists, uninstalls it silently (`uninstall.exe /S`) and
checks that the state survived, then creates the pre-release. The runner is elevated, so this start grants nothing:
native identity opens are refused there. A dispatch that fails before the last step leaves no
release and no tag, so fix and re-dispatch. A run started by pushing a tag leaves that tag behind, and
an interrupted `gh release create` can leave a partial release; clean both up as in *Roll back* before
re-running the same version.

## What the pre-release holds

| Asset | What it is |
|---|---|
| `Loomward_<version>_x64-setup.exe` | NSIS installer: per-user (`%LOCALAPPDATA%`), no administrator rights, **unsigned** (SmartScreen warns) |
| `SHA256SUMS.txt` | SHA-256 of the installer, one `sha256sum` line |
| `Loomward_<version>_x64-setup.exe.sig`, `latest.json` | Only when the updater key is stored (see *The update channels*): the installer's updater signature and the manifest the opt-in updater reads; `latest.json` is also copied to the `preview-latest` pre-release |

The release body (`.github/release/preview-notes.md`) says the build is unsigned; that it runs the
engine on the tester's machine, starts with no folders and reads only folders added through the
Windows folder picker (*Grants & health*, *Add a folder*), metadata only; that file contents are
read only for duplicates on a folder the tester added and for a backup receipt the tester picks;
that learning is local and uses the tester's own labels; what it refuses; that it has no file or
process effect; and where its state lives. It links the evidence index and the issue tracker. The app's
state is under `%LOCALAPPDATA%\Loomward Data\v3\`, a sibling of the per-user install directory
`%LOCALAPPDATA%\Loomward`, so uninstalling removes the program files, not that state. A build that
finds state an earlier build left in `%LOCALAPPDATA%\Loomward\v3\` moves it there once.
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

## Nightly

Owner decisions HUMAN_TODO q-31 and q-33: every push to `main` whose CI run succeeds publishes a
rolling, unsigned **nightly** pre-release ([`.github/workflows/nightly.yml`](.github/workflows/nightly.yml)).
It builds, self-tests, installs and uninstalls exactly as a preview does (both use
[`.github/actions/desktop-installer`](.github/actions/desktop-installer/action.yml)).

- **Version:** `<app version>-nightly.<YYYYMMDD>.<run number>`, for example `0.3.0-nightly.20261012.7`
  (`scripts/release_meta.py nightly-version`). The core comes from `app/package.json`, the same core
  the previews use; the date is UTC; semver compares the two numbers numerically.
- **Tags:** `nightly` always holds the newest build. Before it moves, the build it held is republished
  as `nightly-previous` (its `latest.json` rewritten to point there), so the previous installer stays
  one download away. A re-run on the same commit replaces `nightly` and leaves `nightly-previous` alone.
- **Rollback (manual for now, LW-122):** the app finds the previous nightly but does not install it,
  because an older build refuses state that a newer build migrated, and a rollback has to restore
  the matching state copy with the binary. By hand: close Loomward, install the `nightly-previous`
  installer, and before you start it, in `%LOCALAPPDATA%\Loomward Data\v3\personal`:
  1. If the newer build migrated `state.db` (the older build refuses it with `NewerDatabase`), move
     `state.db` (with `state.db-wal` and `state.db-shm` if present) aside and put the newest
     `state.before-v<N>.db` copy (made by the newer build before its migration) in its place as
     `state.db`.
  2. **Always** move `catalog.db` aside, with `catalog.db-wal` and `catalog.db-shm`, whether or not
     the older build would refuse it. A newer build can change `catalog.db` in ways an older build
     does not refuse but cannot handle: the search index (#438, LW-113) adds triggers on `dir` that a
     build from before it copies onto its own first-scan tables, so that build indexes folders twice
     and then fails on its next open ("already exists"). A schema bump instead gives `NewerDatabase`.
     `catalog.db` is derived data: the older build creates an empty one.

  The cost: the new catalogue has a new `instance_id`, so every object reference in `state.db` is
  marked unresolved until its root is scanned again; sizes, trees and resolved references come back
  only with a re-scan. Restoring `state.before-v<N>.db` also drops whatever the newer build recorded
  after its migration (grants, labels, collections made since).
- **Body:** the preview notes behind a NIGHTLY banner (`.github/release/nightly-banner.md`): untested,
  may break, state is kept and copied before a schema change.
- **Manual run:** `gh workflow run nightly.yml --repo Chris0Jeky/Loomward --ref main`. A run from a
  commit outside `main`'s history is refused.
- **Concurrency:** one publish at a time; a newer waiting run replaces an older waiting one, and a
  running publish is never cancelled half-way.

## The update channels

The desktop's opt-in update check (`native/src/updates.rs`, HUMAN_TODO q-30) reads a Tauri updater
manifest from a fixed URL per channel; the app never asks for one unless the owner presses *Check for
updates* or has turned on *Check on start* (off by default):

| Channel | Manifest |
|---|---|
| preview | `releases/download/preview-latest/latest.json` (the newest preview; `/releases/latest` skips pre-releases) |
| nightly | `releases/download/nightly/latest.json` |
| previous nightly | `releases/download/nightly-previous/latest.json` |

A manifest, and the installer's `.sig`, exist only when the repository holds the updater signing key.
Without it both workflows log a notice and publish the installer alone, and a build whose
`plugins.updater.pubkey` (in `native/tauri.conf.json`) is empty answers "updates are not configured in
this build" without making any request.

### Owner: create the updater key (once)

The updater key signs update metadata so the app installs only builds you published. It is not a
code-signing certificate and does not stop the SmartScreen warning.

1. Generate it on your own machine, outside any repository, with a password:

   ```powershell
   # in a Loomward checkout that has run `npm.cmd --prefix app ci`
   node app\node_modules\@tauri-apps\cli\tauri.js signer generate -w "$HOME\.tauri\loomward-updater.key"
   ```

   This writes `loomward-updater.key` (private) and `loomward-updater.key.pub` (public).
2. Store the private key and its password as repository secrets:

   ```powershell
   Get-Content "$HOME\.tauri\loomward-updater.key" -Raw | gh secret set TAURI_SIGNING_PRIVATE_KEY --repo Chris0Jeky/Loomward
   gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --repo Chris0Jeky/Loomward   # paste the password when asked
   ```

3. Back up the private key file and the password offline (a password manager plus an offline copy).
   If they are lost, every installed build must be reinstalled by hand to trust a new key.
4. Send the contents of `loomward-updater.key.pub` to the driver, who puts it in
   `native/tauri.conf.json` as `plugins.updater.pubkey` in a reviewed PR. Builds from that commit on
   can check for updates; earlier installs (such as `v0.3.0-preview.1`) must be updated by hand once.

The next nightly or preview after step 2 publishes `latest.json` and the `.sig`.
