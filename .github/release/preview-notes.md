**Loomward {{VERSION}} is an unsigned preview of the Windows desktop shell.** It is a pre-release for
early testers, built from commit `{{SHA}}`.

## Read this first

- **Unsigned.** The installer has no code-signing certificate, so Windows SmartScreen will warn
  ("Windows protected your PC"). Choose *More info*, then *Run anyway*, only after checking the
  SHA-256 below. Code signing is a later owner decision.
- **It runs the real engine, on your machine.** The app opens a personal session on its own
  engine and reads metadata only, never file contents: the folders you choose in the Windows folder
  picker (names, sizes, times, file identity), and, when you open the views that show them, your
  installed-applications list, where your known folders (Desktop, Documents and so on) are
  configured, and running-process and disk activity counters. For scanning it refuses whole user profiles,
  credential and browser stores, AppData, network and removable drives, and any folder that holds
  its own state (so not `C:\` itself). Run it as your normal user: started as administrator, it
  refuses every folder.
- **Not in this build: a button to open the picker.** The interface cannot open the folder picker
  yet, so a fresh install shows an empty personal session and scans nothing. Treat this preview as a
  look at the shell, the views and the safety rails, not as a working scanner.
- **Duplicates and learning stay local, and are off for your files.** Duplicate inspection, which
  would read file contents, and learning from your feedback are gated off in personal sessions in
  this build. Nothing is sent anywhere: no cloud, no teacher model, no network requests, no model
  downloads.
- **Read-only, simulation-only.** Loomward never moves, deletes, renames or writes your files, and
  never kills, suspends, reprioritises or trims your processes. The only files it writes are its own: its state under `%LOCALAPPDATA%\Loomward\v3\` and the WebView2
  browser data under `%LOCALAPPDATA%\dev.loomward.desktop\`. Uninstalling leaves the state folder;
  delete it to remove the app's state.
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
