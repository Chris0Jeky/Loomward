**Loomward {{VERSION}} is an unsigned preview of the Windows desktop shell.** It is a pre-release for
early testers, built from commit `{{SHA}}`.

## Read this first

- **Unsigned.** The installer has no code-signing certificate, so Windows SmartScreen will warn
  ("Windows protected your PC"). Choose *More info*, then *Run anyway*, only after checking the
  SHA-256 below. Code signing is a later owner decision.
- **It runs the real engine, on your machine.** The app opens a personal session on its own
  engine. A fresh install starts with no folders (folders you added earlier come back while its state
  folder is kept). To give it one, open *Grants & health*, choose *Add a folder*,
  pick it in the Windows folder picker and confirm (Cancel is the default). Adding checks the folder
  itself (its path, filesystem and identity); nothing inside it is listed until you press *Scan* on
  that folder. A scan reads metadata only, never file contents: names, sizes,
  times and file identity. When you open the views that show them, it also reads your
  installed-applications list, where your known folders (Desktop, Documents and so on) are
  configured, and running-process and disk activity counters.
- **Folders it refuses.** It will not take whole user profiles, credential and browser stores,
  AppData, network or removable drives, links or cloud placeholders, or any folder that holds, or
  sits inside, its own state folder. A whole drive is allowed when you pick it yourself, except the
  drive that holds its state folder (normally `C:\`). Run it as your normal user: started as
  administrator, it refuses every folder and the applications and known-folder reads.
- **FAT and exFAT drives** can be added after an extra confirmation. They are listed read-only and
  marked identity-unverified, because those filesystems cannot vouch for a file's identity.
- **Content is read in one place only: duplicates.** *Duplicates* compares file contents on this
  machine, locally and read-only, only for a folder you added and only when you press *Start
  inspection*. It goes by size, then a partial hash, then a full hash and a byte comparison, within
  fixed budgets (at most 200 candidate files, 64 MiB and 10 s per inspection). Cloud placeholders are skipped, never downloaded. Nothing is deleted
  or moved; equal content is an observation, not advice to delete. The other files it reads are a
  *restic* receipt (and optionally its check output) you pick in the *Backups* view: read-only, at most 256 KiB, and only the
  normalised evidence is stored.
- **Learning stays local.** *Learning review* learns from the labels you give, on this machine,
  using metadata features of the folders you added. Its suggestions are advice with relative
  scores, never an approval, and they never move or delete anything. The optional teacher is
  synthetic-only and is not available in this build. Nothing is sent anywhere: no cloud, no teacher,
  no model downloads, and Loomward itself makes no network requests.
- **Unknown is shown as unknown.** Partial scans, unreadable names, offline items and unverified
  results stay visible as such; missing data is never replaced with zero.
- **Read-only, simulation-only.** Loomward never moves, deletes, renames or writes your files, and
  never kills, suspends, reprioritises or trims your processes. While a scan or a duplicate
  inspection runs, renaming a scanned folder or writing a file being compared may be refused
  briefly ("in use"); this ends when it finishes. The *Grants & health* view may still show
  "content read: off" under its capability tags; that tag is stale (duplicates do read contents, as
  above). The only files it writes are its
  own: its state under `%LOCALAPPDATA%\Loomward Data\v3\` and the WebView2 browser data under
  `%LOCALAPPDATA%\dev.loomward.desktop\`. *Revoke* stops it reading a folder; *Forget* on a revoked
  folder then removes its name and catalogue from Loomward's own database and, where it can, from its
  backup copies (a locked backup copy can keep the name). Your
  files are not touched, and traces may remain in unused disk space until it is overwritten.
  Uninstalling leaves the state folder; delete it to remove the app's state. One Loomward window
  runs at a time: a second start says so and exits.
- **Per-user install.** No administrator rights are needed. The installer fetches nothing from the
  network unless Microsoft's WebView2 runtime is missing from your PC.

## Verify the download

`SHA256SUMS.txt` is attached. In PowerShell, in the folder holding the installer:

```powershell
Get-FileHash .\Loomward_{{VERSION}}_x64-setup.exe -Algorithm SHA256
```

The hash must equal the line in `SHA256SUMS.txt`.

## Evidence and reporting

- Evidence index for this version: [evidence/v3/README.md](https://github.com/Chris0Jeky/Loomward/blob/{{TAG}}/evidence/v3/README.md)
- Report problems at <https://github.com/Chris0Jeky/Loomward/issues>. Do not attach real file names,
  paths or inventories from your machine; this is a public repository.

## Licence

AGPL-3.0-only ([LICENSE](https://github.com/Chris0Jeky/Loomward/blob/{{TAG}}/LICENSE)); earlier versions were MIT. Commercial licences are available from the maintainer through GitHub. The name is covered by [TRADEMARKS.md](https://github.com/Chris0Jeky/Loomward/blob/{{TAG}}/TRADEMARKS.md).
