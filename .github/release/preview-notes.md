**Loomward {{VERSION}} is an unsigned preview of the Windows desktop shell.** It is a pre-release for
early testers, built from commit `{{SHA}}`.

## Read this first

- **Unsigned.** The installer has no code-signing certificate, so Windows SmartScreen will warn
  ("Windows protected your PC"). Choose *More info*, then *Run anyway*, only after checking the
  SHA-256 below. Code signing is a later owner decision.
- **Synthetic data only.** The desktop shell answers from the project's contract example fixtures. It
  does not scan your disk yet, and nothing you see is a measurement of your files.
- **Read-only, simulation-only.** Loomward never moves, deletes, renames or writes your files, and
  never kills, suspends, reprioritises or trims your processes. The installer writes the app's own
  files under `%LOCALAPPDATA%`, and the WebView2 runtime keeps its own profile and cache there. The
  app's code makes no network requests and downloads no models.
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
